use crate::BWErrors::{FsNotFound, FsRoot};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::{env, fmt, fs};

// expected binary name, to work with aliasing
static NOMINAL_BINARY_NAME: &str = "bw";
// grace period to try fallbacking to regular system wide binary launching
static RETRY_GRACE_PERIOD_SEC: u64 = 2;

fn main() {
    let logger = Logger::new();
    let extracted =
        extract_command_and_args(&logger, env::current_exe().ok(), env::args().collect());

    if let Some((command, remaining)) = extracted {
        let gem_file = lookup_gemfile_in_directory(PathBuf::from("."));
        if let Ok(path) = gem_file {
            do_bundle_exec(&logger, &command, &remaining, path);
        } else {
            logger.debug("No Gemfile found, executing global command".to_string());
            let _ = run_bare_command(&command, &remaining);
        }
    }
}

fn extract_command_and_args(
    logger: &Logger,
    current_exe: Option<PathBuf>,
    args: Vec<String>,
) -> Option<(String, Vec<String>)> {
    let invoked_name = current_exe
        .and_then(|p| p.file_name().map(|s| s.to_os_string()))
        .and_then(|f| f.into_string().ok())
        .expect("Can't find binary name");
    logger.debug(format!("invoked_name {}", invoked_name));

    let command: String;
    let start_index: usize;
    if invoked_name == NOMINAL_BINARY_NAME {
        logger.debug("Same command".to_string());
        if args.len() < 2 {
            help();
            return None;
        }
        for arg in &args {
            logger.debug(format!("Arg: {}", arg));
        }
        command = args[1].clone();
        // [bw $command $args]
        start_index = 2;
    } else {
        // remove trailing char
        let mut command_alias = invoked_name;
        command_alias.pop();
        command = command_alias;
        // [$command+ $args]
        start_index = 1
    }
    let remaining: Vec<String> = args[start_index..].to_vec();
    Some((command, remaining))
}

fn do_bundle_exec(logger: &Logger, command: &str, remaining: &[String], path: PathBuf) {
    use std::time::Instant;
    logger.debug(format!(
        "Will execute {} from Gemfile found at {}",
        command,
        path.to_str().unwrap()
    ));
    let fallback_args = &remaining[1..];
    let now = Instant::now();
    let bundle_status = Command::new("bundle")
        .arg("exec")
        .arg(command)
        .args(remaining)
        .status()
        .map_err(|e| {
            logger.fatal(format!("Unable to find bundle in PATH, {}", e));
            run_bare_command(command, fallback_args)
        });
    logger.debug(format!("Status : {}", bundle_status.unwrap()));
    let elapsed = now.elapsed();
    // not sure we should fallback to main command on fail (if it is a lengthy one, we will fail twice)
    // heuristic on duration, if lengthier than x seconds do not ever fall-back
    let _ = bundle_status.map(|status| {
        if !status.success() && elapsed.as_secs() < RETRY_GRACE_PERIOD_SEC {
            run_bare_command(command, fallback_args);
        }
    });
}

fn run_bare_command(command: &str, remaining: &[String]) -> Option<ExitStatus> {
    Command::new(command).args(remaining).status().ok()
}

fn help() {
    println!("bw - Cedric Gatay | Code-Troopers");
    println!();
    println!("Will execute provided command using bundler if available, looking up parent directories recursively for Gemfile");
    println!();
    println!(
        "
If you symlink using another name, it will automatically run the associated binary:
    * `fastlanew` will run `fastlane`
    * `podw repo update` will run `pod repo update`
To prevent conflict, the convention is adding an extra-character to the command.
Here 'w' is used to remind the wrapper thing, but this is not checked extensively"
    );
    println!(
        "
Debug option:
   * if you export BW_DEBUG env var, binary will output all logs"
    );
    //if symlinked to another name, use the symlink name minus the w to execute the bundle command podw > pod, fastlanew > fastlane
}

fn lookup_gemfile_in_directory(path: PathBuf) -> Result<PathBuf, BWErrors> {
    let path = path.as_path();
    if let Ok(paths) = fs::read_dir(path) {
        let gemfile_in_dir = paths
            .flatten()
            .filter(|p| p.file_name().eq(&"Gemfile"))
            .count()
            > 0;
        if gemfile_in_dir {
            Ok(path.to_path_buf())
        } else if let Some(parent_path) = path.parent() {
            lookup_gemfile_in_directory(parent_path.to_path_buf())
        } else {
            Err(FsRoot)
        }
    } else {
        Err(FsNotFound)
    }
}

struct Logger {
    verbose: bool,
}

impl Logger {
    fn new() -> Logger {
        Logger {
            verbose: env::var("BW_DEBUG").map(|_| true).unwrap_or(false),
        }
    }

    fn debug(&self, msg: String) {
        if self.verbose {
            println!("{}", msg);
        }
    }

    fn fatal(&self, msg: String) {
        eprintln!("{}", msg);
    }
}

#[derive(Debug, PartialEq)]
enum BWErrors {
    FsRoot,
    FsNotFound,
}

impl Display for BWErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FsRoot => write!(f, "Reached FS Root"),
            FsNotFound => write!(f, "Unable to read directory"),
        }
    }
}

impl Error for BWErrors {}

#[cfg(test)]
mod test {
    use crate::{
        extract_command_and_args, lookup_gemfile_in_directory, BWErrors, Logger,
        NOMINAL_BINARY_NAME,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn test_root_fs_returns_err() {
        let res = lookup_gemfile_in_directory(PathBuf::from("/"));
        let err = res.unwrap_err();
        assert_eq!(err, BWErrors::FsRoot)
    }

    #[test]
    fn test_unreadable_dir_returns_err() {
        let res = lookup_gemfile_in_directory(PathBuf::from("/non/existent/dir"));
        let err = res.unwrap_err();
        assert_eq!(err, BWErrors::FsNotFound)
    }

    #[test]
    fn test_find_in_current_directory() {
        let res = lookup_gemfile_in_directory(PathBuf::from("./test/root"));
        let found = res.unwrap();
        assert_eq!(found.as_path(), Path::new("./test/root"))
    }

    #[test]
    fn test_find_by_walking_parent_directories() {
        let res =
            lookup_gemfile_in_directory(PathBuf::from("./test/root/nested/directory/deep/inside"));
        let found = res.unwrap();
        assert_eq!(found.as_path(), Path::new("./test/root"))
    }

    #[test]
    fn test_extract_command_same_name_no_args_display_help() {
        let res = extract_command_and_args(
            &Logger::new(),
            Some(PathBuf::from(NOMINAL_BINARY_NAME)),
            vec![String::from(NOMINAL_BINARY_NAME)],
        );
        assert!(res.is_none());
    }

    #[test]
    fn test_extract_command_same_name_propagate_args() {
        let (command, args) = extract_command_and_args(
            &Logger::new(),
            Some(PathBuf::from(NOMINAL_BINARY_NAME)),
            vec![
                String::from(NOMINAL_BINARY_NAME),
                String::from("my"),
                String::from("tailor"),
                String::from("is"),
                String::from("rich"),
            ],
        )
        .unwrap();

        assert_eq!(command, "my");
        assert_eq!(3, args.len());
    }

    #[test]
    fn test_extract_different_same_name_remove_last_char_and_keep_all_args() {
        let (command, args) = extract_command_and_args(
            &Logger::new(),
            Some(PathBuf::from("runw")),
            vec![
                String::from("runw"),
                String::from("my"),
                String::from("tailor"),
                String::from("is"),
                String::from("rich"),
            ],
        )
        .unwrap();

        assert_eq!(command, "run");
        assert_eq!(4, args.len());
        assert_eq!("my", args[0]);
    }
}
