//! Observer admission verifies the same durable process authority as recovery.
//! A pane label or numeric PID alone never authorizes an attachment.
use super::*;

impl<S: StoreAccess> HostManager<S> {
    pub async fn terminal(&self, run: &str, id: &str) -> Result<Value> {
        let attempt = self.store(|s| {
            s.get_run(run)?;
            let value = s.get_attempt(id)?;
            if value["runId"] != run {
                return Err(conflict("Attempt does not belong to this run."));
            }
            Ok(value)
        })?;
        let pane = text(&attempt, "pane")?;
        if !numbered(pane, '%') {
            return Err(conflict("Attempt has no exact terminal pane."));
        }
        let ownership = self.owned_state(&attempt).await?;
        if !matches!(ownership["state"].as_str(), Some("live" | "stopped")) {
            return Err(conflict("Terminal process ownership is not verified."));
        }
        // Socket identity is private, not inherited from the desktop's environment.
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let socket = std::fs::symlink_metadata(&self.0.options.socket)?;
        if !socket.file_type().is_socket()
            || socket.uid() != unsafe { libc::geteuid() }
            || socket.mode() & 0o007 != 0
        {
            return Err(conflict("Terminal socket is not private and owned."));
        }
        let format =
            "#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{window_id}\t#{window_name}\t#{session_name}";
        let target = format!("crewroom:.{pane}");
        let row = self
            .tmux_output(&["display-message", "-p", "-t", &target, format], false)
            .await?
            .ok_or_else(|| conflict("Terminal pane is unavailable."))?;
        let fields: Vec<_> = row.trim_end().split('\t').collect();
        let window = verify_pane(&attempt, &ownership, &fields)?;
        // Recheck after the external observation, before publishing connection data.
        self.store(|s| {
            if s.get_attempt(id)? != attempt {
                return Err(conflict("Attempt changed while verifying its terminal."));
            }
            Ok(())
        })?;
        Ok(
            json!({"version":1,"runId":run,"attemptId":id,"pane":pane,"window":window,
            "session":"crewroom","tmux":self.0.options.tmux,"socket":self.0.options.socket,
            "directory":self.0.options.root,"state":ownership["state"],"readOnly":true,
            "socketDevice":socket.dev(),"socketInode":socket.ino(),
            "hostPid":attempt["hostPid"],"ompPid":attempt["ompPid"],"runnerKind":attempt["runnerKind"],"runnerPid":attempt["runnerPid"]}),
        )
    }
}
fn numbered(value: &str, prefix: char) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}
fn verify_pane<'a>(attempt: &Value, ownership: &Value, fields: &[&'a str]) -> Result<&'a str> {
    if fields.len() != 6
        || fields[0] != text(attempt, "pane")?
        || fields[1].parse::<i64>().ok() != attempt["hostPid"].as_i64()
        || fields[2]
            != if ownership["state"] == "live" {
                "0"
            } else {
                "1"
            }
        || !numbered(fields[3], '@')
        || fields[4] != text(attempt, "id")?
        || fields[5] != "crewroom"
    {
        return Err(conflict(
            "Terminal pane does not match the verified attempt.",
        ));
    }
    Ok(fields[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_identity_requires_exact_pane_host_window_session_and_liveness() {
        let attempt = json!({"id":"attempt","pane":"%3","hostPid":123});
        let row = ["%3", "123", "0", "@2", "attempt", "crewroom"];
        assert_eq!(
            verify_pane(&attempt, &json!({"state":"live"}), &row).unwrap(),
            "@2"
        );
        for (index, value) in [
            (0, "%4"),
            (1, "124"),
            (2, "1"),
            (3, "2"),
            (4, "other"),
            (5, "other"),
        ] {
            let mut bad = row;
            bad[index] = value;
            assert!(verify_pane(&attempt, &json!({"state":"live"}), &bad).is_err());
        }
        assert!(verify_pane(&attempt, &json!({"state":"stopped"}), &row).is_err());
        let mut stopped = row;
        stopped[2] = "1";
        assert!(verify_pane(&attempt, &json!({"state":"stopped"}), &stopped).is_ok());
        for id in ["%", "%1; kill-server", "1", "@2"] {
            assert!(!numbered(id, '%'));
        }
    }
}
