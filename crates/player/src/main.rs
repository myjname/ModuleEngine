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
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            let executable = module_engine_player::package::PLAYER_EXECUTABLE;
            println!(
                "ModuleEngine standalone Player\n  ./{executable}\n  ./{executable} --verify-package <package-root> [output-folder]\n  ./{executable} --headless-check <package-root> [output-folder]\n  ./{executable} --benchmark <output-folder>"
            );
            Ok(())
        }
        Some("--verify-package" | "--headless-check") => {
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
            benchmark(&own_root()?, output)
        }
        None => play(&own_root()?),
        _ => Err("Unknown Player option; use --help".into()),
    }
}
#[cfg(feature = "ui")]
fn play(root: &Path) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        module_engine_player::package::build_package_host(root, false, true)
    })?;
    module_engine_player::run_host(host)
}
#[cfg(not(feature = "ui"))]
fn play(_: &Path) -> Result<(), String> {
    Err("This Player was built without UI; use --headless-check or --verify-package".into())
}
#[cfg(feature = "ui")]
fn benchmark(root: &Path, output: &Path) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        module_engine_player::package::build_package_host(root, false, true)
    })?;
    module_engine_player::run_benchmark_host(host, output)
}
#[cfg(not(feature = "ui"))]
fn benchmark(_: &Path, _: &Path) -> Result<(), String> {
    Err("Native window benchmark requires the ui feature".into())
}
