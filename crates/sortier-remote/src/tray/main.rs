//! Sortier Remote in the Windows notification area: it runs the server (`sortier-remote serve`, as a process of its
//! own) with no terminal window, shows whether it's listening, restarts it if it crashes, and can start itself when
//! you sign in. Setting it up (the password, the folders) stays with the `sortier-remote` command.

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
    use std::collections::VecDeque;
    use std::io::{BufRead as _, Write as _};
    use std::net::SocketAddr;
    use std::os::windows::process::CommandExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{ChildStdin, Command, Stdio};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, anyhow, bail};
    use clap::Parser;
    use sortier_remote::said;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    use crate::{autostart, icon, system};

    /// The log is started afresh when it grows past this.
    const LOG_LIMIT: u64 = 1024 * 1024;
    /// How long the server gets to stop (finishing requests, saving the library) before it's ended.
    const STOP_WAIT: Duration = Duration::from_secs(10);
    /// How often the server is checked on, to notice it has ended.
    const EXIT_CHECK: Duration = Duration::from_millis(200);
    /// How long after a crash the server is started again.
    const RESTART_DELAY: Duration = Duration::from_secs(5);
    /// A server that crashes this many times within `CRASH_WINDOW` is left stopped rather than restarted again.
    const CRASH_LIMIT: usize = 3;
    const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);
    /// How many of the server's latest error lines are kept, to say why it stopped.
    const ERROR_LINES: usize = 20;

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
        let mut app = App::new(data, server_program()?)?;
        log_crashes(app.log.clone());
        app.start();
        system::run_message_loop(|| app.handle_events());
        Ok(())
    }

    /// `sortier-remote.exe`, which must be next to this program: it's what runs the server.
    fn server_program() -> Result<PathBuf> {
        let program = std::env::current_exe().context("this program's path can't be found")?.with_file_name("sortier-remote.exe");
        if !program.is_file() {
            bail!(
                "sortier-remote.exe isn't in {}. Keep it next to sortier-remote-tray.exe: the tray app runs the server with it.",
                program.parent().map_or_else(String::new, |folder| folder.display().to_string())
            );
        }
        Ok(program)
    }

    /// Writes where and why the tray app panics into the log, which is otherwise lost: there's no terminal to print to.
    fn log_crashes(log: Arc<Log>) {
        std::panic::set_hook(Box::new(move |panic| {
            log.write(&format!("The tray app crashed: {panic}\n{}", std::backtrace::Backtrace::force_capture()));
        }));
    }

    /// What the server's threads tell the tray. `run` says which start of the server it's about, so news of one
    /// that has since been replaced is ignored.
    enum News {
        /// A line the server printed: to its error output when `error`.
        Said { run: u64, line: String, error: bool },
        /// The server ended, with its exit code when it has one.
        Ended { run: u64, code: Option<i32> },
        /// The server was asked to stop `STOP_WAIT` ago.
        StopOverdue { run: u64 },
        /// It's time to start the server again after a crash.
        RestartDue,
    }

    /// What to do once the server has stopped.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Then {
        Nothing,
        Restart,
        Quit,
    }

    /// The server process, while it runs.
    struct Running {
        run: u64,
        /// Its standard input; closing it asks the server to stop. `None` once it has been asked.
        input: Option<ChildStdin>,
        /// Ends the process outright.
        kill: mpsc::Sender<()>,
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
        program: PathBuf,
        log: Arc<Log>,
        main_thread: u32,
        news: mpsc::Sender<News>,
        heard: mpsc::Receiver<News>,
        /// How many times the server has been started.
        runs: u64,
        running: Option<Running>,
        fingerprint: Option<String>,
        /// The server's latest error lines, to say why it stopped.
        errors: VecDeque<String>,
        /// When the server crashed recently, to stop restarting one that keeps crashing.
        crashes: VecDeque<Instant>,
        /// When the server last crashed and why, as the status shows it.
        last_crash: Option<String>,
        restart_pending: bool,
        then: Then,
        menus: Menus,
        tray: TrayIcon,
    }

    impl App {
        fn new(data: PathBuf, program: PathBuf) -> Result<Self> {
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
                program,
                main_thread: system::current_thread(),
                news,
                heard,
                runs: 0,
                running: None,
                fingerprint: None,
                errors: VecDeque::new(),
                crashes: VecDeque::new(),
                last_crash: None,
                restart_pending: false,
                then: Then::Nothing,
                menus,
                tray,
            })
        }

        /// A way for other threads to send news, waking the message loop to handle it.
        fn teller(&self) -> impl Fn(News) + Send + Clone + 'static {
            let (news, main_thread) = (self.news.clone(), self.main_thread);
            move |said: News| {
                let _ = news.send(said);
                system::wake(main_thread);
            }
        }

        /// Starts `sortier-remote serve` with no window. Threads pass on what it prints and when it ends.
        fn start(&mut self) {
            self.runs += 1;
            let run = self.runs;
            self.then = Then::Nothing;
            self.restart_pending = false;
            self.errors.clear();
            self.set_status("Starting\u{2026}");
            self.log.write("Starting the server.");
            let spawned = Command::new(&self.program)
                .arg("--data")
                .arg(&self.data)
                .args(["serve", "--until-input-closes"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(error) => {
                    let message = format!("{} can't be started: {error}", self.program.display());
                    self.log.write(&message);
                    self.set_status("Stopped");
                    system::message_box(&message, system::Kind::Error);
                    return;
                }
            };
            let tell = self.teller();
            let readers: Vec<_> = [
                child.stdout.take().map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
                child.stderr.take().map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(stream, output)| Some((stream == 1, output?)))
            .map(|(error, output)| {
                let tell = tell.clone();
                std::thread::spawn(move || {
                    for line in std::io::BufReader::new(output).lines().map_while(Result::ok) {
                        tell(News::Said { run, line, error });
                    }
                })
            })
            .collect();
            let (kill, killed) = mpsc::channel::<()>();
            let input = child.stdin.take();
            std::thread::spawn(move || {
                let code = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break status.code(),
                        Ok(None) => {}
                        Err(_) => break None,
                    }
                    if killed.try_recv().is_ok() {
                        let _ = child.kill();
                    }
                    std::thread::sleep(EXIT_CHECK);
                };
                // Everything it printed is passed on before word that it ended.
                for reader in readers {
                    let _ = reader.join();
                }
                tell(News::Ended { run, code });
            });
            self.running = Some(Running { run, input, kill });
        }

        /// Asks the server to stop, then does `then`; does it now when the server isn't running.
        fn stop(&mut self, then: Then) {
            let Some(running) = self.running.as_mut() else {
                return self.after_stop(then);
            };
            self.then = then;
            if running.input.take().is_some() {
                let (run, tell) = (running.run, self.teller());
                std::thread::spawn(move || {
                    std::thread::sleep(STOP_WAIT);
                    tell(News::StopOverdue { run });
                });
                self.set_status("Stopping\u{2026}");
            }
        }

        fn after_stop(&mut self, then: Then) {
            match then {
                Then::Quit => system::quit_message_loop(),
                Then::Restart => {
                    self.crashes.clear();
                    self.last_crash = None;
                    self.start();
                }
                Then::Nothing => {}
            }
        }

        fn set_status(&self, status: &str) {
            self.menus.status.set_text(status);
            let _ = self.tray.set_tooltip(Some(format!("Sortier Remote: {}", status.to_lowercase())));
        }

        /// Handles whatever the menu and the server's threads have said since last time.
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

        fn menu_chosen(&mut self, id: &MenuId) {
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
            let current = self.running.as_ref().map(|running| running.run);
            match news {
                News::Said { run, line, error } => {
                    self.log.write(&line);
                    if Some(run) == current {
                        self.said(line, error);
                    }
                }
                News::StopOverdue { run } => {
                    if let Some(running) = self.running.as_ref().filter(|running| running.run == run) {
                        self.log.write(&format!("The server didn't stop within {} seconds, so it was ended.", STOP_WAIT.as_secs()));
                        let _ = running.kill.send(());
                    }
                }
                News::RestartDue => {
                    if self.restart_pending && self.running.is_none() {
                        self.start();
                    }
                }
                News::Ended { run, code } => {
                    if Some(run) == current {
                        self.ended(code);
                    }
                }
            }
        }

        /// Follows what the running server prints: when it's listening, its fingerprint, and its errors.
        fn said(&mut self, line: String, error: bool) {
            if let Some(address) = line.strip_prefix(said::LISTENING).and_then(|address| address.parse::<SocketAddr>().ok()) {
                let status = match &self.last_crash {
                    Some(crash) => format!("Listening on port {}, restarted after {crash}", address.port()),
                    None => format!("Listening on port {}", address.port()),
                };
                self.set_status(&status);
            } else if let Some(fingerprint) = line.strip_prefix(said::FINGERPRINT) {
                self.fingerprint = Some(fingerprint.to_string());
                self.menus.fingerprint.set_enabled(true);
            } else if line == said::NO_FOLDERS {
                system::message_box(
                    "No folders are served yet. Add one in a terminal with sortier-remote add-folder <path>; the server starts looking through it within a few seconds.",
                    system::Kind::Info,
                );
            } else if error {
                if self.errors.len() == ERROR_LINES {
                    self.errors.pop_front();
                }
                self.errors.push_back(line);
            }
        }

        /// The server ended with `code`: as asked, with an error it explained, or by crashing.
        fn ended(&mut self, code: Option<i32>) {
            let Some(running) = self.running.take() else { return };
            self.fingerprint = None;
            self.menus.fingerprint.set_enabled(false);
            let then = std::mem::replace(&mut self.then, Then::Nothing);
            let asked = running.input.is_none();
            // `main` returning an error prints "Error: …" and exits with 1.
            let explained = self.errors.iter().rposition(|line| line.starts_with(said::ERROR));
            if asked {
                self.log.write("Stopped.");
                self.set_status("Stopped");
                self.after_stop(then);
            } else if let (Some(1), Some(from)) = (code, explained) {
                let why: Vec<&str> = self.errors.iter().skip(from).map(String::as_str).collect();
                let why = why.join("\n");
                let why = why.strip_prefix(said::ERROR).unwrap_or(&why);
                self.log.write("Stopped, for the reason above.");
                self.set_status("Stopped");
                system::message_box(
                    &format!("Sortier Remote isn't serving:\n\n{why}\n\nPut it right, then choose Restart Server from its menu."),
                    system::Kind::Error,
                );
            } else {
                self.crashed(code);
            }
        }

        /// Restarts a server that crashed, unless it keeps crashing; says so either way.
        fn crashed(&mut self, code: Option<i32>) {
            let how = describe_exit(code);
            self.log.write(&format!("The server crashed ({how}). What it printed last is above."));
            let now = Instant::now();
            self.crashes.retain(|at| now.duration_since(*at) < CRASH_WINDOW);
            self.crashes.push_back(now);
            let time = system::local_time();
            self.last_crash = Some(format!("a crash at {}", time.get(11..16).unwrap_or(&time)));
            if self.crashes.len() < CRASH_LIMIT {
                self.set_status("Restarting after a crash\u{2026}");
                self.restart_pending = true;
                let tell = self.teller();
                std::thread::spawn(move || {
                    std::thread::sleep(RESTART_DELAY);
                    tell(News::RestartDue);
                });
            } else {
                self.set_status("Stopped after crashing");
                system::message_box(
                    &format!(
                        "Sortier Remote's server crashed {} times in {} minutes, most recently with {how}, so it has been left stopped.\n\nOpen Log from its menu shows what it was doing. Restart Server tries again.",
                        self.crashes.len(),
                        CRASH_WINDOW.as_secs() / 60
                    ),
                    system::Kind::Error,
                );
            }
        }

        fn open(&self, path: &Path) {
            if let Err(error) = Command::new("explorer.exe").arg(path).spawn() {
                system::message_box(&format!("{} can't be opened: {error}", path.display()), system::Kind::Error);
            }
        }
    }

    /// A process's exit code in words: Windows' name for the exception that ended it, where it's a common one.
    fn describe_exit(code: Option<i32>) -> String {
        let Some(code) = code else { return "no exit code".to_string() };
        let status = code as u32;
        let name = match status {
            0xC000_0005 => Some("a memory access violation"),
            0xC000_0017 | 0xC000_012D => Some("too little memory"),
            0xC000_001D => Some("an illegal instruction"),
            0xC000_0094 => Some("a division by zero"),
            0xC000_00FD => Some("a stack overflow"),
            0xC000_0374 => Some("heap corruption"),
            0xC000_0409 => Some("an abort"),
            // What Stop-Process and other programs ending it outright usually leave.
            0xFFFF_FFFF => Some("being ended by another program"),
            _ => None,
        };
        match name {
            Some(name) => format!("{name}, 0x{status:08X}"),
            None if status >= 0xC000_0000 => format!("exception 0x{status:08X}"),
            None => format!("exit code {code}"),
        }
    }

    /// `sortier-remote.log` in the data folder: what the server printed, and what the tray did about it.
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn exit_codes_are_named() {
            assert_eq!(describe_exit(Some(0xC000_0374_u32 as i32)), "heap corruption, 0xC0000374");
            assert_eq!(describe_exit(Some(0xC000_0420_u32 as i32)), "exception 0xC0000420");
            assert_eq!(describe_exit(Some(101)), "exit code 101");
            assert_eq!(describe_exit(None), "no exit code");
        }
    }
}
