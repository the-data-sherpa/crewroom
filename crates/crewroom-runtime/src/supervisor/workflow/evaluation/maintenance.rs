//! Controller-owned fixture preparation commands have their own durable cleanup.
use super::*;
impl<S: StoreAccess> Workflow<S> {
    pub(super) async fn verify_evaluation_image(&self, run: &str, manifest: &Value) -> Result<()> {
        let root = self
            .0
            .config
            .root
            .join(run)
            .join("tools")
            .join("image-proof");
        private_directory(&root)?;
        let workspace = root.join("source");
        private_directory(&workspace)?;
        let checks = pack()["checks"].clone();
        let names = array(&checks)
            .iter()
            .map(|c| c["path"].clone())
            .collect::<Vec<_>>();
        let script = format!(
            "const f=require('node:fs'),c=require('node:crypto');console.log(JSON.stringify({}.map(p=>({{path:p,sha256:c.createHash('sha256').update(f.readFileSync('/opt/crewroom-checks/'+p)).digest('hex')}}))))",
            stringify(&json!(names))
        );
        let engine = self
            .0
            .config
            .engine
            .as_ref()
            .ok_or_else(|| conflict("Contained checks unavailable."))?;
        let job = ContainerCommand::new(root.clone(), Path::new(text(engine, "socket")?))?;
        let spec = json!({"image":manifest["execution"]["image"],"command":"/usr/local/bin/node","args":["-e",script],"timeoutMs":10000,"memoryMiB":128,"readOnlyWorkspace":true,"workspace":workspace});
        if !root.join("container.json").exists() {
            job.start(spec.clone()).await?;
        }
        job.wait().await?;
        let receipt = job.receipt().await?;
        let output = receipt["output"].as_str().unwrap_or("");
        let observed = serde_json::from_str::<Value>(output.trim()).ok();
        job.remove().await?;
        if receipt["exitCode"] != 0
            || receipt["started"] != true
            || observed.as_ref().is_none_or(|v| {
                array(v).len() != array(&checks).len()
                    || array(&checks).iter().any(|c| {
                        !array(v)
                            .iter()
                            .any(|o| o["path"] == c["path"] && o["sha256"] == c["sha256"])
                    })
            })
        {
            return Err(conflict(
                "Pinned image does not contain the frozen protected checks.",
            ));
        }
        if job.inspect().await["state"] != "stopped" {
            return Err(conflict("Image verification cleanup is unresolved."));
        }
        Ok(())
    }
    pub(in crate::supervisor::workflow) async fn stop_evaluation_maintenance(
        &self,
        run: &str,
    ) -> Result<()> {
        if self.store(|s| s.get_run(run))?["purpose"] != "evaluation" {
            return Ok(());
        }
        let root = self.0.config.root.join(run).join("tools");
        if !root.exists() {
            return Ok(());
        }
        crate::canonical(&root)?;
        let engine = self.0.config.engine.as_ref().ok_or_else(|| {
            conflict("Restore contained command engine before evaluation cleanup.")
        })?;
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            crate::canonical(&path)?;
            if !path.join("container.json").exists() {
                continue;
            }
            let job = ContainerCommand::new(path, Path::new(text(engine, "socket")?))?;
            if job.stop().await?["state"] != "stopped" {
                return Err(conflict("Evaluation preparation ownership unresolved."));
            }
            job.remove().await?;
        }
        Ok(())
    }
}
