//! Puts the commit this was built from into the build.
//!
//! The page shows it beside the crate version, so a bug report names a build
//! rather than a version that moved a dozen times. Nothing else reads it, and
//! nothing about the game changes with it.
//!
//! Two ways in, in this order. `TG_GIT_HASH` in the environment wins, which is
//! what the Makefile sets and what a build somewhere with no checkout can be
//! handed. Otherwise `git` is asked, which is what a bare `cargo build` gets.
//! Neither working is not a failure: an unbuilt hash is a footer that says so.

use std::process::Command;

fn main() {
    // The env var first, so the Makefile's answer is the one that counts and a
    // change to it rebuilds. The git fallback below cannot be watched the same
    // way: the file that moves on a commit is the branch's ref rather than
    // HEAD, and following that through packed refs is more machinery than a
    // string in a footer deserves.
    println!("cargo:rerun-if-env-changed=TG_GIT_HASH");
    println!("cargo:rerun-if-changed=.git/HEAD");

    let hash = std::env::var("TG_GIT_HASH")
        .ok()
        .filter(|hash| !hash.trim().is_empty())
        .or_else(from_git)
        .unwrap_or_else(|| "unknown".to_string());

    // Shortened here rather than by whoever handed it over, so one place
    // decides how long a commit is written and every source agrees. CI passes
    // the event's own sha, which is all forty characters of it.
    let short: String = hash.trim().chars().take(SHORT_HASH).collect();
    println!("cargo:rustc-env=TG_GIT_HASH={short}");
}

/// How much of a commit to show. Long enough to be unambiguous in a tree this
/// size, short enough to sit in a footer.
const SHORT_HASH: usize = 9;

fn from_git() -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "--short=9", "HEAD"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let hash = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!hash.is_empty()).then_some(hash)
}
