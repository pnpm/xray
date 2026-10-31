//! Resolve Windows command shims as well as native executables.
use std::{ffi::OsStr, process::Command};

pub fn command(program: &OsStr) -> Command {
    #[cfg(windows)]
    {
        let path = std::path::Path::new(program);
        if path.extension().is_none() {
            let directories = if path.components().count() > 1 {
                vec![std::path::PathBuf::new()]
            } else {
                std::env::var_os("PATH")
                    .map(|value| std::env::split_paths(&value).collect())
                    .unwrap_or_default()
            };
            for directory in directories {
                for extension in ["exe", "cmd", "bat", "com"] {
                    let candidate = directory.join(path).with_extension(extension);
                    if candidate.is_file() {
                        return Command::new(candidate);
                    }
                }
            }
        }
    }
    Command::new(program)
}

#[cfg(all(test, windows))]
#[path = "process/tests.rs"]
mod tests;
