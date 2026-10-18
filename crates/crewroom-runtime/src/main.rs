//! Process helper CLI; all command/descriptor JSON comes from the trusted controller.
use crewroom_core::{Error, Result, authority::Authority};
use crewroom_runtime::{CheckedProcess, CheckedSpec, CommandSpec};
use std::{
    io::{Read, Write},
    os::unix::process::ExitStatusExt,
    path::Path,
};
fn input<T: serde::de::DeserializeOwned>() -> Result<T> {
    Ok(serde_json::from_reader(std::io::stdin())?)
}
fn print(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        let message = format!("Process helper failed: {error}\n");
        let mut end = message.len().min(512);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let _ = std::io::stderr().write_all(&message.as_bytes()[..end]);
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let arg = |i: usize| {
        args.get(i)
            .map(String::as_str)
            .ok_or_else(|| Error::Invalid("Missing process helper argument.".into()))
    };
    match arg(0)? {
        "hold" => {
            let authority = Authority::acquire_for_execution(Path::new(arg(1)?))?;
            print(serde_json::json!({"root":authority.root(),"epoch":authority.epoch()}))?;
            std::io::stdout().flush()?;
            let mut sink = [0u8; 1024];
            while std::io::stdin().read(&mut sink)? > 0 {}
            authority.assert_current()
        }
        "describe" => print(crewroom_runtime::describe_checked(&input::<CheckedSpec>()?)?),
        "validate-launch" => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Launch {
                descriptor: CheckedProcess,
                epoch: String,
                controller_root: std::path::PathBuf,
            }
            let launch: Launch = input()?;
            crewroom_runtime::validate_launch(
                &launch.descriptor,
                &launch.epoch,
                &launch.controller_root,
            )?;
            print(serde_json::json!({"valid":true}))
        }
        "prepare-checked" => print(crewroom_runtime::prepare_checked(&input::<CheckedSpec>()?)?),
        "prepare-owned" => {
            crewroom_runtime::prepare_owned(Path::new(arg(1)?), &input::<CommandSpec>()?)?;
            print(serde_json::json!({"prepared":true}))
        }
        "exec" => crewroom_runtime::helper::execute_owned(Path::new(arg(1)?)),
        operation @ ("checked-exec" | "checked-exec-report" | "checked-owned") => {
            let descriptor = CheckedProcess {
                version: 1,
                root: arg(1)?.into(),
                token: arg(2)?.into(),
                spec_sha256: arg(3)?.into(),
                epoch: arg(4)?.into(),
                controller_root: arg(5)?.into(),
            };
            if operation == "checked-owned" {
                return crewroom_runtime::helper::execute_checked_owned(&descriptor);
            }
            let status = crewroom_runtime::helper::execute_checked(&descriptor)?;
            if operation == "checked-exec-report" {
                // This pipe is a live-parent observation only. It is deliberately
                // separate from retained logs and never used as restart proof.
                let mut output = std::io::stdout().lock();
                writeln!(
                    output,
                    "{}",
                    serde_json::json!({"code":status.code(),"signal":status.signal()})
                )?;
                output.flush()?;
            }
            crewroom_runtime::helper::exit_like(status)
        }
        operation @ ("inspect" | "stop" | "inspect-checked" | "seal") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let state = match operation {
                "inspect" => {
                    runtime.block_on(crewroom_runtime::inspect_owned(Path::new(arg(1)?), true))
                }
                "stop" => runtime.block_on(crewroom_runtime::stop_owned(Path::new(arg(1)?))),
                "inspect-checked" => runtime.block_on(crewroom_runtime::inspect_checked(&input()?)),
                _ => runtime.block_on(crewroom_runtime::seal_checked(&input()?)),
            };
            print(state)
        }
        _ => Err(Error::Invalid("Unknown process helper operation.".into())),
    }
}
