use sonata_launcher_core::{
    instance::model::InstanceId,
    modloader::model::{LoaderKind, LoaderSpec},
};

pub(super) const USAGE: &str = "\
sonata-launcher-core cli usage:

    java-add <version> <exec_path> <home_path> [vendor]
    java-discover
    java-list
    create <name> <version> <manifest_url> <provider>
    create-modded <name> <mc> <loader> <loader-version> <manifest-url> <provider>
    register-cleanroom <release-descriptor.json>
    register-cleanroom-url <loader-version> <official-mmc-zip-url> <java-majors-comma-separated>
    set-java <instance_id> <java_id>
    recovery-list <instance_id>
    recovery-retry <instance_id>
    recovery-abandon <instance_id> <operation_id> --keep-files
    install <instance_id>
    launch <instance_id>
    list
    help | --help | -h
";

#[derive(Debug, thiserror::Error)]
#[error("{0}\n\n{USAGE}")]
pub(crate) struct ParseError(pub(super) String);

pub(super) enum Command<'a> {
    Help,
    JavaAdd {
        version: &'a str,
        exec: &'a str,
        home: &'a str,
        vendor: Option<&'a str>,
    },
    JavaDiscover,
    JavaList,
    Create {
        name: &'a str,
        version: &'a str,
        loader: LoaderSpec,
        url: &'a str,
        provider: &'a str,
    },
    RegisterCleanroom(&'a str),
    RegisterCleanroomUrl {
        version: &'a str,
        url: &'a str,
        majors: Vec<u32>,
    },
    SetJava {
        instance: InstanceId,
        java: i64,
    },
    RecoveryList(InstanceId),
    RecoveryRetry(InstanceId),
    RecoveryAbandon {
        instance: InstanceId,
        operation: uuid::Uuid,
    },
    Install(InstanceId),
    Launch(InstanceId),
    List,
}

fn id(raw: &str) -> Result<i64, ParseError> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| ParseError(format!("invalid positive ID: {raw:?}")))
}

fn url(raw: &str) -> Result<&str, ParseError> {
    let parsed =
        http_types::Url::parse(raw).map_err(|error| ParseError(format!("invalid URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ParseError(
            "URL must use HTTP or HTTPS and have a host".into(),
        ));
    }
    Ok(raw)
}

/// Pure argument parsing. No config, env, files, services or tasks are touched.
pub(super) fn parse<'a>(argv: &[&'a str]) -> Result<Command<'a>, ParseError> {
    Ok(match argv {
        [] | ["help" | "--help" | "-h"] => Command::Help,
        ["java-add", version, exec, home, rest @ ..] if rest.len() <= 1 => Command::JavaAdd {
            version,
            exec,
            home,
            vendor: rest.first().copied(),
        },
        ["java-discover"] => Command::JavaDiscover,
        ["java-list"] => Command::JavaList,
        ["create", name, version, manifest, provider] => Command::Create {
            name,
            version,
            loader: LoaderSpec::vanilla(),
            url: url(manifest)?,
            provider,
        },
        ["create-modded", name, version, loader, loader_version, manifest, provider] => {
            let kind = LoaderKind::ALL
                .into_iter()
                .find(|kind| kind.as_str() == *loader)
                .ok_or_else(|| ParseError(format!("unknown loader: {loader}")))?;
            let loader = LoaderSpec {
                kind,
                version: Some((*loader_version).into()),
            };
            loader
                .validate(version)
                .map_err(|error| ParseError(error.to_string()))?;
            Command::Create {
                name,
                version,
                loader,
                url: url(manifest)?,
                provider,
            }
        }
        ["register-cleanroom", descriptor] => Command::RegisterCleanroom(descriptor),
        ["register-cleanroom-url", version, archive, majors] => {
            let majors: Vec<u32> = majors
                .split(',')
                .map(|major| {
                    major
                        .parse::<u32>()
                        .ok()
                        .filter(|major| *major > 0)
                        .ok_or_else(|| ParseError(format!("invalid Java major: {major:?}")))
                })
                .collect::<Result<_, _>>()?;
            Command::RegisterCleanroomUrl {
                version,
                url: url(archive)?,
                majors,
            }
        }
        ["set-java", instance, java] => Command::SetJava {
            instance: id(instance)?.into(),
            java: id(java)?,
        },
        ["recovery-list", instance] => Command::RecoveryList(id(instance)?.into()),
        ["recovery-retry", instance] => Command::RecoveryRetry(id(instance)?.into()),
        ["recovery-abandon", instance, operation, "--keep-files"] => Command::RecoveryAbandon {
            instance: id(instance)?.into(),
            operation: operation
                .parse()
                .map_err(|error| ParseError(format!("invalid operation ID: {error}")))?,
        },
        ["install", instance] => Command::Install(id(instance)?.into()),
        ["launch", instance] => Command::Launch(id(instance)?.into()),
        ["list"] => Command::List,
        _ => return Err(ParseError("unknown command or incorrect arguments".into())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_documented_create_shape_parse_without_a_loader_argument() {
        for args in [vec![], vec!["help"], vec!["--help"], vec!["-h"]] {
            assert!(matches!(parse(&args), Ok(Command::Help)));
        }
        assert!(USAGE.contains("create <name> <version> <manifest_url> <provider>"));
        assert!(
            matches!(parse(&["create", "test", "1.20.1", "https://example.invalid/manifest", "prism"]), Ok(Command::Create { loader, .. }) if loader.kind == LoaderKind::Vanilla)
        );
        assert!(parse(&[
            "create",
            "test",
            "1.20.1",
            "vanilla",
            "https://example.invalid/manifest",
            "prism"
        ])
        .is_err());
    }

    #[test]
    fn argument_errors_are_rejected_before_a_command_can_reach_dispatch() {
        for args in [
            vec!["java-add", "21", "java", ".", "vendor", "extra"],
            vec!["install", "nope"],
            vec!["launch", "0"],
            vec!["set-java", "1", "-2"],
            vec!["recovery-abandon", "1", "invalid", "--keep-files"],
            vec![
                "recovery-abandon",
                "1",
                "00000000-0000-0000-0000-000000000001",
            ],
            vec![
                "create-modded",
                "test",
                "1.20.1",
                "unknown",
                "1",
                "https://example.invalid",
                "prism",
            ],
            vec!["create", "test", "1", "not a URL", "prism"],
            vec![
                "register-cleanroom-url",
                "1",
                "https://example.invalid",
                "8,broken",
            ],
            vec!["list", "extra"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }
}
