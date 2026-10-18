//! Test-only JSON transport to the production container boundary.
use crewroom_runtime::container_command::ContainerCommand;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
};
#[tokio::main]
async fn main() {
    let result = async {
        let mut bytes = String::new();
        std::io::stdin().read_to_string(&mut bytes)?;
        let v: Value = serde_json::from_str(&bytes)?;
        let command = ContainerCommand::new(
            PathBuf::from(v["root"].as_str().unwrap()),
            std::path::Path::new(v["socket"].as_str().unwrap()),
        )?;
        let result = match v["op"].as_str().unwrap() {
            "start" => {
                command.start(v["spec"].clone()).await?;
                Value::Null
            }
            "inspect" => command.inspect().await,
            "stop" => command.stop().await?,
            "wait" => command.wait().await?,
            "logs" => json!(command.logs().await?),
            "receipt" => command.receipt().await?,
            "remove" => {
                command.remove().await?;
                Value::Null
            }
            "follow" => {
                command
                    .follow(
                        |bytes| {
                            std::io::stdout().write_all(bytes)?;
                            std::io::stdout().flush()?;
                            Ok(())
                        },
                        tokio_util::sync::CancellationToken::new(),
                    )
                    .await?;
                return Ok::<_, crewroom_core::Error>(());
            }
            _ => panic!("Unknown fixture operation"),
        };
        println!("{}", crewroom_core::serialized::stringify(&result));
        Ok(())
    }
    .await;
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
