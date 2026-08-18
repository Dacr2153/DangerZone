use std::process::Command;

pub const RELEASE_PAGE: &str = "https://github.com/freedomofpress/dangerzone/releases/latest";

pub fn open_release_page() {
    let _ = open_url(RELEASE_PAGE);
}

pub fn open_url(url: &str) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
