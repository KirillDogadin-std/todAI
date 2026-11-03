//! Desktop notifications for agent posts that arrive while the operator is
//! looking elsewhere.

/// One `notify-send` listing `posts` (at most five, then a count) under the
/// board `title`. Silently does nothing without `notify-send`.
pub fn desktop(title: &str, posts: &[String]) {
    if posts.is_empty() {
        return;
    }
    let mut body: Vec<String> = posts.iter().take(5).cloned().collect();
    if posts.len() > 5 {
        body.push(format!("and {} more", posts.len() - 5));
    }
    let summary = format!("💀 todai · {title}");
    let child = std::process::Command::new("notify-send")
        .args(["--app-name=todai", &summary, &body.join("\n")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Reap it off-thread so no zombie is left behind.
    if let Ok(mut c) = child {
        std::thread::spawn(move || c.wait());
    }
}
