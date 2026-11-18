use std::path::PathBuf;

use sonata_launcher_core::{
    instance::settings::SettingsPatch,
    java::model::{JavaSource, NewJavaRuntime},
    Config, GlobalState,
};

mod parse;
mod progress;
pub(crate) use parse::ParseError;
use parse::{Command, USAGE};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let command = parse::parse(&argv)?;
    if matches!(command, Command::Help) {
        print!("{USAGE}");
        return Ok(());
    }

    dotenvy::dotenv().ok();

    let config = Config {
        database_url: std::env::var("DATABASE_URL").ok(),
        curseforge_api_key: std::env::var("CURSEFORGE_API_KEY").ok(),
        ..Config::init()
    };

    let state = GlobalState::new(config).await?;

    let reporter = progress::spawn_reporter(&state.bus);
    let outcome = dispatch(&state, command).await;

    state.shutdown().await;

    reporter.finish().await;

    outcome
}

async fn dispatch(
    state: &GlobalState,
    command: Command<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::JavaAdd {
            version,
            exec,
            home,
            vendor,
        } => {
            let java = state
                .add_java(NewJavaRuntime {
                    version: Some(version.to_string()),
                    major: None,
                    arch: None,
                    exec_path: Some(PathBuf::from(exec)),
                    home_path: Some(PathBuf::from(home)),
                    vendor: vendor.map(str::to_owned),
                    source: JavaSource::Manual,
                })
                .await?;

            println!("java runtime registered: id={} {}", java.id, java.version);
        }

        Command::JavaDiscover => {
            let added = state.rediscover_java().await?;
            println!("java runtimes found: {added}");
        }

        Command::JavaList => {
            for java in state.java().list_all().await? {
                println!(
                    "{:>3}  java {:<3} {:<14} {:<8} {:<10} {}",
                    java.id,
                    java.major,
                    java.version,
                    java.arch.as_deref().unwrap_or("?"),
                    java.source.as_str(),
                    java.exec_path,
                );
            }
        }

        Command::Create {
            name,
            version,
            loader,
            url,
            provider,
        } => {
            let rec = state
                .instances()
                .create_with_loader(
                    name.into(),
                    version.into(),
                    loader,
                    url.into(),
                    provider.into(),
                )
                .await?;

            println!("instance created: id={} dir={}", rec.id, rec.dir);
        }

        Command::RegisterCleanroomUrl {
            version,
            url,
            majors,
        } => {
            state
                .modloaders()
                .import_cleanroom_archive(version.into(), url.into(), majors)
                .await?;
            println!("Cleanroom release registered from archive");
        }
        Command::RegisterCleanroom(descriptor) => {
            let bytes = async_std::fs::read(descriptor).await?;
            let release = serde_json::from_slice(&bytes)?;
            state.modloaders().register_cleanroom(release).await?;
            println!("Cleanroom release registered");
        }
        Command::SetJava { instance, java } => {
            let view = state
                .options()
                .patch_settings(
                    instance,
                    SettingsPatch {
                        java_runtime: Some(Some(java)),
                        ..Default::default()
                    },
                )
                .await?;

            println!("effective settings: {:#?}", view.effective);
        }

        Command::RecoveryList(id) => {
            for operation in state.content().pending_recovery(id).await? {
                println!("{operation}");
            }
        }

        Command::RecoveryRetry(id) => {
            state.content().retry_recovery(id).await?;
        }

        Command::RecoveryAbandon {
            instance,
            operation,
        } => {
            state
                .content()
                .abandon_recovery(instance, &operation.to_string())
                .await?;
            println!("Recovery abandoned. Existing files were kept.");
        }

        Command::Install(id) => {
            state.instances().install(id).await?;
            println!("install finished");
        }

        Command::Launch(id) => {
            state.instances().launch(id).await?;
        }

        Command::List => {
            let instances = state.instances().list().await?;

            if instances.is_empty() {
                println!("no installed instances");
            } else {
                for instance in instances {
                    println!("{}", instance.name);
                }
            }
        }

        Command::Help => unreachable!("help is handled before bootstrap"),
    }

    Ok(())
}
