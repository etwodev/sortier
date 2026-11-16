//! Sortier Remote in the Windows notification area: it runs the server with no terminal window, shows whether it's
//! listening, and can start itself when you sign in. Setting it up (the password, the folders) stays with the
//! `sortier-remote` command.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod icon;
#[cfg(windows)]
mod system;

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "sortier-remote-tray is for Windows. Elsewhere, run `sortier-remote serve` as a service: `sortier-remote --help` says how."
    );
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        system::message_box(&format!("{error:#}"), system::Kind::Error);
    }
}

#[cfg(windows)]
mod windows {
    use std::io::Write as _;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, mpsc};

    use anyhow::{Result, anyhow};
    use clap::Parser;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

    use crate::{autostart, icon, system};

    /// The log is started afresh when it grows past this.
    const LOG_LIMIT: u64 = 1024 * 1024;

    #[derive(Parser)]
    #[command(name = "sortier-remote-tray", version, about = "Sortier Remote in the notification area")]
    struct Cli {
        /// The server's data folder, as given to `sortier-remote --data` (or set SORTIER_REMOTE_DIR).
        #[arg(long)]
        data: Option<PathBuf>,
    }

    pub fn run() -> Result<()> {
        // There's no terminal to print to, so even --help and --version arrive as a message box.
        let cli = Cli::try_parse().map_err(|error| anyhow!("{}", error.render()))?;
        let data = std::path::absolute(sortier_remote::open_data_folder(cli.data)?)?;
        let Some(_instance) = system::SingleInstance::claim(&data)? else {
            system::message_box("Sortier Remote is already running: look for its heart in the notification area.", system::Kind::Info);
            return Ok(());
        };
        let mut app = App::new(data)?;
        app.start();
        system::run_message_loop(|| app.handle_events());
        Ok(())
    }

    /// What the server thread tells the tray.
    enum News {
        Listening { address: SocketAddr, fingerprint: String, folders: usize },
        Stopped(Result<()>),
    }

    /// What to do once the server has stopped.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Then {
        Nothing,
        Restart,
        Quit,
    }

    struct Menus {
        status: MenuItem,
        fingerprint: MenuItem,
        open_folder: MenuItem,
        open_log: MenuItem,
        start_with_windows: CheckMenuItem,
        restart: MenuItem,
        quit: MenuItem,
    }

    struct App {
        data: PathBuf,
        log: Arc<Log>,
        main_thread: u32,
        news: mpsc::Sender<News>,
        heard: mpsc::Receiver<News>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        fingerprint: Option<String>,
        then: Then,
        menus: Menus,
        tray: TrayIcon,
    }

    impl App {
        fn new(data: PathBuf) -> Result<Self> {
            let menus = Menus {
                status: MenuItem::new("Starting\u{2026}", false, None),
                fingerprint: MenuItem::new("Show Fingerprint\u{2026}", false, None),
                open_folder: MenuItem::new("Open Data Folder", true, None),
                open_log: MenuItem::new("Open Log", true, None),
                start_with_windows: CheckMenuItem::new("Start with Windows", true, autostart::is_on(&data), None),
                restart: MenuItem::new("Restart Server", true, None),
                quit: MenuItem::new("Quit", true, None),
            };
            let menu = Menu::new();
            menu.append_items(&[
                &menus.status,
                &PredefinedMenuItem::separator(),
                &menus.fingerprint,
                &menus.open_folder,
                &menus.open_log,
                &PredefinedMenuItem::separator(),
                &menus.start_with_windows,
                &menus.restart,
                &menus.quit,
            ])?;
            let tray = TrayIconBuilder::new()
                .with_icon(icon::heart())
                .with_tooltip("Sortier Remote: starting")
                .with_menu(Box::new(menu))
                .build()?;
            let (news, heard) = mpsc::channel();
            Ok(Self {
                log: Arc::new(Log::open(&data)),
                data,
                main_thread: system::current_thread(),
                news,
                heard,
                stop: None,
                fingerprint: None,
                then: Then::Nothing,
                menus,
                tray,
            })
        }

        /// Runs the server on a thread of its own, which reports back through `news` and wakes the message loop.
        fn start(&mut self) {
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            self.stop = Some(stop);
            self.then = Then::Nothing;
            self.set_status("Starting\u{2026}");
            let (data, log, news, main_thread) = (self.data.clone(), self.log.clone(), self.news.clone(), self.main_thread);
            std::thread::spawn(move || {
                let tell = |said: News| {
                    let _ = news.send(said);
                    system::wake(main_thread);
                };
                let logging = log.clone();
                let result = sortier_remote::serve(
                    &data,
                    |listening| {
                        tell(News::Listening {
                            address: listening.address,
                            fingerprint: listening.fingerprint.clone(),
                            folders: listening.folders,
                        })
                    },
                    move |message| logging.write(message),
                    async move {
                        let _ = stopped.await;
                    },
                );
                tell(News::Stopped(result));
            });
        }

        /// Asks the server to stop, then does `then`; does it now when the server isn't running.
        fn stop(&mut self, then: Then) {
            match self.stop.take() {
                Some(stop) => {
                    let _ = stop.send(());
                    self.then = then;
                    self.set_status("Stopping\u{2026}");
                }
                None => self.after_stop(then),
            }
        }

        fn after_stop(&mut self, then: Then) {
            match then {
                Then::Quit => system::quit_message_loop(),
                Then::Restart => self.start(),
                Then::Nothing => {}
            }
        }

        fn set_status(&self, status: &str) {
            self.menus.status.set_text(status);
            let _ = self.tray.set_tooltip(Some(format!("Sortier Remote: {}", status.to_lowercase())));
        }

        /// Handles whatever the menu and the server thread have said since last time.
        fn handle_events(&mut self) {
            // Clicks and hovers on the icon itself aren't used: the menu opens on its own.
            while TrayIconEvent::receiver().try_recv().is_ok() {}
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                self.menu_chosen(event.id());
            }
            while let Ok(news) = self.heard.try_recv() {
                self.news_heard(news);
            }
        }

        fn menu_chosen(&mut self, id: &tray_icon::menu::MenuId) {
            let menus = &self.menus;
            if id == menus.fingerprint.id() {
                if let Some(fingerprint) = &self.fingerprint {
                    let grouped: Vec<String> = fingerprint.as_bytes().chunks(4).map(|chunk| String::from_utf8_lossy(chunk).to_uppercase()).collect();
                    system::message_box(
                        &format!(
                            "When a device connects, Sortier shows the server's fingerprint. Make sure it's this one:\n\n{}\n\n(Ctrl+C copies this message.)",
                            grouped.join(" ")
                        ),
                        system::Kind::Info,
                    );
                }
            } else if id == menus.open_folder.id() {
                self.open(&self.data);
            } else if id == menus.open_log.id() {
                self.open(&self.log.path);
            } else if id == menus.start_with_windows.id() {
                let on = autostart::is_on(&self.data);
                match autostart::set(!on, &self.data) {
                    Ok(()) => self.menus.start_with_windows.set_checked(!on),
                    Err(error) => {
                        self.menus.start_with_windows.set_checked(on);
                        system::message_box(&format!("Sortier Remote can't change whether it starts with Windows: {error:#}"), system::Kind::Error);
                    }
                }
            } else if id == menus.restart.id() {
                self.stop(Then::Restart);
            } else if id == menus.quit.id() {
                self.stop(Then::Quit);
            }
        }

        fn news_heard(&mut self, news: News) {
            match news {
                News::Listening { address, fingerprint, folders } => {
                    self.log.write(&format!("Listening on {address}. Certificate fingerprint: {fingerprint}"));
                    self.fingerprint = Some(fingerprint);
                    self.menus.fingerprint.set_enabled(true);
                    self.set_status(&format!("Listening on port {}", address.port()));
                    if folders == 0 {
                        let message = "No folders are served yet. Add one in a terminal with sortier-remote add-folder <path>; the server picks it up at its next scan.";
                        self.log.write(message);
                        system::message_box(message, system::Kind::Info);
                    }
                }
                News::Stopped(result) => {
                    self.stop = None;
                    self.fingerprint = None;
                    self.menus.fingerprint.set_enabled(false);
                    let then = std::mem::replace(&mut self.then, Then::Nothing);
                    match result {
                        Err(error) if then == Then::Nothing => {
                            self.log.write(&format!("Stopped: {error:#}"));
                            self.set_status("Stopped");
                            system::message_box(
                                &format!("Sortier Remote isn't serving:\n\n{error:#}\n\nPut it right, then choose Restart Server from its menu."),
                                system::Kind::Error,
                            );
                        }
                        _ => {
                            self.log.write("Stopped.");
                            self.set_status("Stopped");
                        }
                    }
                    self.after_stop(then);
                }
            }
        }

        fn open(&self, path: &Path) {
            if let Err(error) = std::process::Command::new("explorer.exe").arg(path).spawn() {
                system::message_box(&format!("{} can't be opened: {error}", path.display()), system::Kind::Error);
            }
        }
    }

    /// `sortier-remote.log` in the data folder: what the server would have printed to a terminal.
    struct Log {
        path: PathBuf,
        file: Mutex<Option<std::fs::File>>,
    }

    impl Log {
        fn open(data: &Path) -> Self {
            let path = data.join("sortier-remote.log");
            let too_big = std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > LOG_LIMIT);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(!too_big)
                .write(true)
                .truncate(too_big)
                .open(&path)
                .ok();
            Self { path, file: Mutex::new(file) }
        }

        fn write(&self, message: &str) {
            let mut file = self.file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(file) = file.as_mut() {
                let _ = writeln!(file, "[{}] {message}\r", system::local_time());
            }
        }
    }
}
