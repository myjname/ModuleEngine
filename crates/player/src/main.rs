use std::{
    fs,
    path::{Path, PathBuf},
};

fn main() {
    let result = run();
    let cleanup = module_engine_host::game_worker::finish_after_ui();
    #[cfg(feature = "ui")]
    let gpu_cleanup = module_engine_player::gpu_presentation::finish_gpu_retirements()
        .map(|_| ())
        .map_err(|error| error.to_string());
    #[cfg(not(feature = "ui"))]
    let gpu_cleanup = Ok(());
    if let Err(error) = result.and(cleanup).and(gpu_cleanup) {
        eprintln!("ModuleEngine Player: {error}");
        std::process::exit(1);
    }
}
fn own_root() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Player executable has no package folder".into())
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let state_root = take_state_root(&mut args)?;
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            let executable = module_engine_player::package::PLAYER_EXECUTABLE;
            println!(
                "ModuleEngine standalone Player\n  ./{executable} [--state-root <profile-folder>]\n  ./{executable} --verify-package <package-root> [output-folder]\n  ./{executable} --headless-check <package-root> [output-folder]\n  ./{executable} --benchmark <output-folder> [--state-root <profile-folder>]"
            );
            Ok(())
        }
        Some("--verify-package" | "--headless-check") => {
            if state_root.is_some() {
                return Err("--state-root applies to interactive play and --benchmark only".into());
            }
            let root = Path::new(args.get(1).ok_or("Provide the Player package root")?);
            let output = args.get(2).map(Path::new);
            let report = module_engine_player::package::verify_package(
                root,
                output,
                args[0] == "--headless-check",
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        Some("--benchmark") => {
            let output = Path::new(args.get(1).ok_or("Provide a benchmark output folder")?);
            fs::create_dir_all(output).map_err(|error| error.to_string())?;
            benchmark(&own_root()?, output, state_root.as_deref())
        }
        None => play(&own_root()?, state_root.as_deref()),
        _ => Err("Unknown Player option; use --help".into()),
    }
}
fn take_state_root(args: &mut Vec<String>) -> Result<Option<PathBuf>, String> {
    let mut selected = None;
    while let Some(index) = args.iter().position(|arg| arg == "--state-root") {
        if selected.is_some() {
            return Err("Provide --state-root only once".into());
        }
        let path = args
            .get(index + 1)
            .filter(|path| !path.is_empty() && !path.starts_with("--"))
            .ok_or("Provide a profile folder after --state-root")?;
        selected = Some(PathBuf::from(path));
        args.drain(index..=index + 1);
    }
    Ok(selected)
}
#[cfg(feature = "ui")]
fn play(root: &Path, state_root: Option<&Path>) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        module_engine_player::package::build_package_host_with_state_root(
            root, false, true, state_root,
        )
    })?;
    module_engine_player::run_host(host)
}
#[cfg(not(feature = "ui"))]
fn play(_: &Path, _: Option<&Path>) -> Result<(), String> {
    Err("This Player was built without UI; use --headless-check or --verify-package".into())
}
#[cfg(feature = "ui")]
fn benchmark(root: &Path, output: &Path, state_root: Option<&Path>) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        module_engine_player::package::build_package_host_with_state_root(
            root, false, true, state_root,
        )
    })?;
    module_engine_player::run_benchmark_host(host, output)
}
#[cfg(not(feature = "ui"))]
fn benchmark(_: &Path, _: &Path, _: Option<&Path>) -> Result<(), String> {
    Err("Native window benchmark requires the ui feature".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_profile_location_is_extracted_without_consuming_command_arguments() {
        let mut args = [
            "--benchmark",
            "measurements",
            "--state-root",
            "profiles with spaces",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            take_state_root(&mut args).unwrap(),
            Some(PathBuf::from("profiles with spaces"))
        );
        assert_eq!(args, ["--benchmark", "measurements"]);
        assert_eq!(take_state_root(&mut args).unwrap(), None);
    }

    #[test]
    fn missing_or_ambiguous_profile_roots_are_rejected() {
        for values in [
            vec!["--state-root"],
            vec!["--state-root", "--benchmark"],
            vec!["--state-root", "a", "--state-root", "b"],
        ] {
            assert!(take_state_root(&mut values.into_iter().map(String::from).collect()).is_err());
        }
    }
}
