use std::{path::PathBuf, process::exit};

use sonata_launcher_core::{
    data::GlobalState,
    instance::{model::InstanceId, settings::SettingsPatch},
    java::model::NewJavaRuntime,
    Config,
};

mod progress;

const USAGE: &str = "\
sonata-launcher-core usage:

    java-add <version> <exec_path> <home_path> [vendor]
    create   <name> <version> <loader> <manifest_url> <provider>
    set-java <instance_id> <java_id>
    install  <instance_id>
    launch   <instance_id>
    list
";

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();

    let config = Config {
        database_url: std::env::var("DATABASE_URL").ok(),
        ..Config::init()
    };

    let state = GlobalState::new(config).await?;

    let reporter = progress::spawn_reporter(&state.bus);
    let outcome = dispatch(&state, &argv).await;

    reporter.finish().await;

    outcome
}

async fn dispatch(state: &GlobalState, argv: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    match argv {
        ["java-add", version, exec_path, home_path, rest @ ..] => {
            let java = state
                .java()
                .init_external(NewJavaRuntime {
                    version: (*version).to_string(),
                    exec_path: PathBuf::from(exec_path),
                    home_path: PathBuf::from(home_path),
                    vendor: rest.first().map(|v| (*v).to_string()),
                })
                .await?;

            println!("java runtime registered: id={} {}", java.id, java.version);
        }

        ["create", name, version, loader, manifest_url, provider] => {
            let rec = state
                .instances()
                .create(
                    (*name).to_string(),
                    (*version).to_string(),
                    (*loader).to_string(),
                    (*manifest_url).to_string(),
                    (*provider).to_string(),
                )
                .await?;

            println!("instance created: id={} dir={}", rec.id, rec.dir);
        }

        ["set-java", id, java_id] => {
            let view = state
                .options()
                .patch_settings(
                    parse_id(id)?,
                    SettingsPatch {
                        java_runtime: Some(Some(java_id.parse()?)),
                        ..Default::default()
                    },
                )
                .await?;

            println!("effective settings: {:#?}", view.effective);
        }

        ["install", id] => {
            state.instances().install(parse_id(id)?).await?;
            println!("install finished");
        }

        ["launch", id] => {
            state.instances().launch(parse_id(id)?).await?;
        }

        ["list"] => {
            let instances = state.instances().list().await?;

            if instances.len() <= 0 {
                println!("no installed instances");
            } else {
                for instance in instances {
                    println!("{}", instance.name);
                }
            }
        }

        _ => {
            eprint!("{USAGE}");
            exit(2);
        }
    }

    Ok(())
}

fn parse_id(raw: &str) -> Result<InstanceId, std::num::ParseIntError> {
    raw.parse::<i64>().map(InstanceId::from)
}
