//! G2: launching Chrome on the host (guard_rules.py `_g2`, `_open_launches_chrome`).

use anthology_pycompat::str as pystr;
use anthology_shell::Entry;

use super::SEARCH_PROGRAMS;

/// `_chrome_app_name(name)`.
fn chrome_app_name(name: &str) -> bool {
    let name = pystr::lower(name);
    name.contains("chrome") || name.contains("chromium")
}

/// `posixpath.basename(path)`.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `_open_launches_chrome(args)`: `open -a <Chrome>`, `open -b com.google.Chrome...` or a
/// Chrome `.app` path.
fn open_launches_chrome(args: &[String]) -> bool {
    let mut k = 0;
    while k < args.len() {
        let token = args[k].as_str();
        let value = args.get(k + 1).map_or("", String::as_str);
        let last = token.chars().last();
        if token.starts_with('-')
            && !token.starts_with("--")
            && pystr::char_len(token) > 1
            && matches!(last, Some('a' | 'b'))
            && token[1..].chars().all(pystr::is_alpha)
        {
            if last == Some('a') && chrome_app_name(value) {
                return true;
            }
            let bundle = pystr::lower(value);
            if last == Some('b')
                && (bundle.starts_with("com.google.chrome") || bundle.starts_with("org.chromium."))
            {
                return true;
            }
            k += 2;
            continue;
        }
        if !token.starts_with('-') {
            let lowered = pystr::lower(token);
            let app = lowered.trim_end_matches('/');
            if app.ends_with(".app") && chrome_app_name(basename(app)) {
                return true;
            }
        }
        k += 1;
    }
    false
}

/// `_g2(entry)`.
pub fn g2(entry: &Entry) -> bool {
    if entry.exempt {
        return false;
    }
    let prog = entry.prog.as_str();
    if [
        "google-chrome",
        "chromium",
        "google chrome",
        "chrome-headless-shell",
    ]
    .iter()
    .any(|p| prog.starts_with(p))
        || ["chrome", "chromedriver", "chrome-launcher"].contains(&prog)
        || entry.prog_path.contains("google chrome.app/")
        || entry.prog_path.contains("chromium.app/")
    {
        return true;
    }
    if prog == "open" && open_launches_chrome(&entry.args) {
        return true;
    }
    if prog == "osascript"
        && entry.args.iter().any(|a| {
            let a = pystr::lower(a);
            a.contains("google chrome") || a.contains("chromium")
        })
    {
        return true;
    }
    if !SEARCH_PROGRAMS.contains(&prog) {
        return entry
            .raw
            .iter()
            .any(|t| t.starts_with("--remote-debugging-port"));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn open_reads_app_and_bundle_options() {
        assert!(open_launches_chrome(&args(&["-a", "Google Chrome"])));
        assert!(open_launches_chrome(&args(&["-na", "Chromium"])));
        assert!(open_launches_chrome(&args(&["-b", "com.google.Chrome"])));
        assert!(open_launches_chrome(&args(&["/Applications/Google Chrome.app/"])));
        assert!(!open_launches_chrome(&args(&["-a", "Safari"])));
        // `-a`'s value is consumed, so a Chrome path right after it is the app's name
        assert!(!open_launches_chrome(&args(&["-a", "x", "-g"])));
        assert!(open_launches_chrome(&args(&["--args", "chrome.app"])));
    }
}
