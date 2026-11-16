//! Starting the tray app when you sign in to Windows, through your own Run key (no administrator needed).

use std::path::Path;

use anyhow::{Context as _, Result};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Sortier Remote";

/// The command Windows runs at sign-in: this program, serving `data`.
fn command(data: &Path) -> Result<String> {
    let program = std::env::current_exe().context("this program's path can't be found")?;
    Ok(format!("{} --data {}", quoted(&program), quoted(data)))
}

/// `path` in double quotes, with a trailing backslash doubled so it doesn't escape the closing quote.
fn quoted(path: &Path) -> String {
    let path = path.display().to_string();
    if path.ends_with('\\') { format!("\"{path}\\\"") } else { format!("\"{path}\"") }
}

/// Whether Windows starts this program, serving `data`, at sign-in.
pub fn is_on(data: &Path) -> bool {
    let Ok(command) = command(data) else { return false };
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(VALUE))
        .is_ok_and(|value| value.eq_ignore_ascii_case(&command))
}

/// Has Windows start this program, serving `data`, at sign-in, or stop doing so.
pub fn set(on: bool, data: &Path) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY).context("the Run key can't be opened")?;
    if on {
        key.set_value(VALUE, &command(data)?).context("the Run key can't be written")
    } else {
        match key.delete_value(VALUE) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error).context("the Run key can't be written"),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_quoted_so_a_trailing_backslash_stays_inside() {
        assert_eq!(quoted(Path::new(r"C:\Sortier Remote")), r#""C:\Sortier Remote""#);
        assert_eq!(quoted(Path::new(r"D:\")), r#""D:\\""#);
    }
}
