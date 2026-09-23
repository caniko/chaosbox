//! Process boundary for the bundled Python transcription engine.
use std::{ffi::OsString, io, process::Command};

fn command(args: &[OsString]) -> Command {
    let mut command = Command::new(
        std::env::var_os("CHAOSBOX_CANSCRIBE_BIN").unwrap_or_else(|| "canscribe".into()),
    );
    command.args(args);
    command
}

pub fn run(args: &[OsString]) -> io::Result<i32> {
    let status = command(args).status()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    #[test]
    fn forwards_paths_and_flags_without_shell_interpretation() {
        let args = [
            "audio with spaces;$(false).wav",
            "--visual",
            "--output",
            "out.txt",
        ]
        .map(std::ffi::OsString::from);
        let command = super::command(&args);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args.iter()
                .map(std::ffi::OsString::as_os_str)
                .collect::<Vec<_>>()
        );
    }
}
