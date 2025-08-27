#![windows_subsystem = "windows"]
mod utils;
use iced::alignment::Vertical;
use iced::futures::channel::{mpsc, oneshot};
use iced::futures::executor::block_on;
use iced::futures::{SinkExt, StreamExt};
use iced::widget::{
    Column, Scrollable, Space, button, checkbox, column, container, progress_bar, row, text,
};
use iced::{Color, Element, Length, Task, Theme};
use rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rfd::FileHandle;
use std::fs::{DirBuilder, File};
use std::io::BufReader;
use std::path::PathBuf;

pub fn main() -> iced::Result {
    let init = || {
        initial_path()
            .map(Message::Selected)
            .map(Task::done)
            .unwrap_or_else(Task::none)
    };
    iced::application(
        move || (State::default(), init()),
        State::update,
        State::view,
    )
    .theme(State::theme)
    .title("Asset Bundle Extractor")
    .window_size((800., 600.))
    .run()
}

fn initial_path() -> Option<PathBuf> {
    /*Some(PathBuf::from(
        "/home/jakob/.local/share/Steam/steamapps/common/Nine Sols/NineSols_Data/data.unity3d",
    ))*/
    None
}

#[derive(Debug, Clone)]
struct Selection {
    bundle_files: Vec<(String, usize)>,
}

struct State {
    path: Option<PathBuf>,
    selection: Option<Selection>,
    skip_resources: bool,

    error: Option<String>,
    export_progress: Option<f32>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            path: None,
            selection: None,
            error: None,
            skip_resources: true,
            export_progress: None,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    OpenPicker,
    Selected(PathBuf),
    Export,
    SetProgress(Option<f32>),
    SetSkipResources(bool),
    Error(String),
}

impl State {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenPicker => Task::future(async {
                let file = rfd::AsyncFileDialog::new()
                    .set_title("Open a text file...")
                    .add_filter("AssetBundle", &["unity3d", "bundle", "assetbundle"])
                    .pick_file()
                    .await;

                file.map(FileHandle::into)
            })
            .and_then(|path| Task::done(Message::Selected(path))),
            Message::Selected(path) => {
                self.path = Some(path);

                // TODO async
                let selection = self
                    .path
                    .as_ref()
                    .map(|handle| -> std::io::Result<_> {
                        let file = BufReader::new(File::open(handle)?);
                        let mut reader =
                            BundleFileReader::from_reader(file, &ExtractionConfig::default())?;
                        let mut bundle_files = Vec::new();
                        while let Some(file) = reader.next() {
                            bundle_files.push((file.path, file.size));
                        }
                        bundle_files.sort_by(|a, b| numeric_sort::cmp(&a.0, &b.0));
                        Ok(Selection { bundle_files })
                    })
                    .transpose();

                match selection {
                    Ok(selection) => {
                        self.selection = selection;
                        Task::none()
                    }
                    Err(e) => Task::done(Message::Error(e.to_string())),
                }
            }
            Message::SetSkipResources(skip) => {
                self.skip_resources = skip;
                Task::none()
            }
            Message::Error(error) => {
                self.error = Some(error);
                Task::none()
            }
            Message::Export => {
                self.error = None;

                let mut path = self.path.as_ref().cloned();
                let skip_resources = self.skip_resources;

                Task::done(Message::SetProgress(Some(0.0))).chain(
                    Task::future(
                        rfd::AsyncFileDialog::new()
                            .set_title("Output folder")
                            .pick_folder(),
                    )
                    .then(move |out_dir| {
                        let Some(out_dir) = out_dir else {
                            return Task::done(Message::SetProgress(None));
                        };

                        let (progress_receiver, result) =
                            export_bundle(path.take().unwrap(), out_dir.into(), skip_resources);

                        Task::stream(progress_receiver.map(Message::SetProgress)).chain(
                            Task::future(result).then(|res| match res {
                                Ok(_) => Task::none(),
                                Err(e) => Task::done(Message::Error(e)),
                            }),
                        )
                    }),
                )
            }
            Message::SetProgress(progress) => {
                self.export_progress = progress;
                Task::none()
            }
        }
    }

    fn should_take(&self, path: &str) -> bool {
        if self.skip_resources {
            return !is_resource(path);
        }
        true
    }

    fn view(&self) -> Element<'_, Message> {
        match &self.path {
            Some(path) => {
                let column = self
                    .selection
                    .as_ref()
                    .map(|selection| {
                        selection
                            .bundle_files
                            .iter()
                            .map(|file| {
                                let text = text(format!(
                                    "- {} ({})",
                                    file.0,
                                    utils::friendly_size(file.1)
                                ))
                                .color_maybe(
                                    (!self.should_take(&file.0))
                                        .then_some(Color::from_rgb8(153, 163, 173)),
                                );
                                Element::from(text)
                            })
                            .collect()
                    })
                    .unwrap_or(Column::new());
                let scrollable = Scrollable::new(column)
                    .width(Length::Fill)
                    .height(Length::Fill);

                let can_run = self.export_progress.is_none();

                let filename = path.file_name().unwrap();
                let mut display_filename = filename.display().to_string();
                display_filename.truncate(40);

                let controls = row![
                    container(
                        button(text(format!("{display_filename}...")))
                            .on_press(Message::OpenPicker)
                    ),
                    match self.export_progress {
                        Some(progress) => Element::new(progress_bar(0.0..=1.0, progress)),
                        None => Element::new(Space::with_width(Length::Fill)),
                    },
                    checkbox("Skip Resources", self.skip_resources)
                        .on_toggle_maybe(can_run.then_some(Message::SetSkipResources)),
                    button("Export").on_press_maybe(can_run.then_some(Message::Export))
                ]
                .spacing(8)
                .align_y(Vertical::Center);

                column![scrollable, controls].spacing(12).padding(12).into()
            }
            None => container(button("Open Assetbundle").on_press(Message::OpenPicker))
                .center(Length::Fill)
                .into(),
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

fn is_resource(path: &str) -> bool {
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    ["assets", "resS"].contains(&ext)
}

fn export_bundle(
    bundle_path: PathBuf,
    out_dir: PathBuf,
    skip_resources: bool,
) -> (
    mpsc::Receiver<Option<f32>>,
    impl Future<Output = Result<(), String>>,
) {
    let (mut tx, rx) = mpsc::channel(1);

    let (oneshot_tx, oneshot_rx) = oneshot::channel::<()>();
    let handle = std::thread::spawn(move || -> std::io::Result<()> {
        let config = ExtractionConfig::default();

        let file = BufReader::new(File::open(bundle_path)?);
        let mut reader = BundleFileReader::from_reader(file, &config)?;

        let total = reader
            .files()
            .iter()
            .filter(|entry| !(skip_resources && is_resource(&entry.path)))
            .count();

        let mut i = 0;
        while let Some(mut file) = reader.next() {
            let out_file_path = out_dir.join(&file.path);

            if skip_resources && is_resource(&file.path) {
                continue;
            }
            let data = file.read()?;

            DirBuilder::new()
                .recursive(true)
                .create(out_file_path.parent().unwrap())?;
            std::fs::write(out_file_path, data)?;

            i += 1;
            let progress = (i + 1) as f32 / total as f32;
            block_on(tx.send(Some(progress))).unwrap();
        }
        block_on(tx.send(None)).unwrap();
        let _ = open::that_detached(out_dir);
        oneshot_tx.send(()).unwrap();

        Ok(())
    });

    (rx, async {
        let _ = oneshot_rx.await;
        let result = handle.join();

        match result {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(e)) => Err(format!("Error during export: {e}")),
            Err(_) => Err("Error during export".into()),
        }
    })
}
