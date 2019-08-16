#[macro_use]
extern crate log;

use snafu::ResultExt;
use std::collections::HashSet;
use std::env;
use std::process;

use thar_be_settings::{config, get_changed_settings, service, settings, template};

// FIXME Get from configuration in the future
const DEFAULT_API_SOCKET: &str = "/var/lib/thar/api.sock";

mod error {
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub(super)")]
    pub(super) enum Error {
        #[snafu(display("Logger setup error: {}", source))]
        Logger { source: log::SetLoggerError },
    }
}

/// RunMode represents how thar-be-settings was requested to be run, either handling all
/// configuration files and services, or handling configuration files and services based on
/// specific keys given by the user.
#[derive(Debug)]
enum RunMode {
    All,
    SpecificKeys,
}

/// Store the args we receive on the command line
struct Args {
    mode: RunMode,
    verbosity: usize,
    socket_path: String,
}

/// Print a usage message in the event a bad arg is passed
fn usage() -> ! {
    let program_name = env::args().next().unwrap_or_else(|| "program".to_string());
    eprintln!(
        r"Usage: {}
            [ --all ]
            [ --socket-path PATH ]
            [ --verbose --verbose ... ]

    If --all is given, all configuration files will be written and all
    services will have their restart-commands run.  Otherwise, settings keys
    will be read from stdin; only files related to those keys will be written,
    and only services related to those keys will be restarted.

    Socket path defaults to {}",
        program_name, DEFAULT_API_SOCKET,
    );
    process::exit(2);
}

/// Prints a more specific message before exiting through usage().
fn usage_msg<S: AsRef<str>>(msg: S) -> ! {
    eprintln!("{}\n", msg.as_ref());
    usage();
}

/// Parse the args to the program and return an Args struct
fn parse_args(args: env::Args) -> Args {
    let mut mode = RunMode::SpecificKeys;
    let mut verbosity = 2;
    let mut socket_path = None;

    let mut iter = args.skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_ref() {
            "--all" => mode = RunMode::All,

            "-v" | "--verbose" => verbosity += 1,

            "--socket-path" => {
                socket_path = Some(
                    iter.next()
                        .unwrap_or_else(|| usage_msg("Did not give argument to --socket-path")),
                )
            }

            _ => usage(),
        }
    }

    Args {
        mode,
        verbosity,
        socket_path: socket_path.unwrap_or_else(|| DEFAULT_API_SOCKET.to_string()),
    }
}

/// Render and write config files to disk.  If `files_limit` is Some, only
/// write those files, otherwise write all known files.
fn write_config_files(
    args: &Args,
    files_limit: Option<HashSet<String>>,
) -> Result<(), Box<std::error::Error>> {
    // Create a vec of ConfigFile structs from the list of changed services
    info!("Requesting configuration file data for affected services");
    let config_files = config::get_affected_config_files(&args.socket_path, files_limit)?;
    trace!("Found config files: {:?}", config_files);

    // Build the template registry from config file metadata
    debug!("Building template registry");
    let template_registry = template::build_template_registry(&config_files)?;

    // Get all settings values for config file templates
    debug!("Requesting settings values");
    let settings = settings::get_settings_from_template(&args.socket_path, &template_registry)?;

    // Ensure all files render properly
    info!("Rendering config files...");
    let rendered = config::render_config_files(&template_registry, config_files, settings)?;

    // If all the config renders properly, write it to disk
    info!("Writing config files to disk...");
    config::write_config_files(rendered)?;

    Ok(())
}

fn main() -> Result<(), Box<std::error::Error>> {
    // Parse and store the args passed to the program
    let args = parse_args(env::args());

    // TODO fix this in the future when we understand our logging strategy;
    // it should also be configurable
    // Start the logger
    stderrlog::new()
        .module(module_path!())
        .timestamp(stderrlog::Timestamp::Millisecond)
        .verbosity(args.verbosity)
        .color(stderrlog::ColorChoice::Never)
        .init()
        .context(error::Logger)?;

    info!("thar-be-settings started");

    match args.mode {
        RunMode::SpecificKeys => {
            // Get the settings that changed via stdin
            info!("Parsing stdin for updated settings");
            let changed_settings = get_changed_settings()?;

            // Create a HashSet of affected services
            info!(
                "Requesting affected services for settings: {:?}",
                &changed_settings
            );
            let services =
                service::get_affected_services(&args.socket_path, Some(changed_settings))?;
            trace!("Found services: {:?}", services);
            if services.is_empty() {
                info!("No services are affected, exiting...");
                process::exit(0)
            }

            // Create a HashSet of configuration file names
            let config_file_names = config::get_config_file_names(&services);

            if !config_file_names.is_empty() {
                write_config_files(&args, Some(config_file_names))?;
            }

            // Now go bounce the affected services
            info!("Restarting affected services...");
            service::restart_services(services)?;
        }
        RunMode::All => {
            write_config_files(&args, None)?;

            info!("Restarting all services...");
            let services = service::get_affected_services(&args.socket_path, None)?;
            trace!("Found services: {:?}", services);
            service::restart_services(services)?;
        }
    }

    Ok(())
}
