use super::*;
#[derive(Clone)]
pub struct Artifacts {
    root: PathBuf,
}
impl Artifacts {
    pub fn new(root: PathBuf) -> Result<Self> {
        crate::create_root(&root)?;
        Ok(Self { root })
    }
    pub fn write(&self, body: &str) -> Result<Value> {
        if body.len() > 4 * 1024 * 1024 {
            return Err(conflict("Evidence artifact exceeds its limit."));
        }
        crate::canonical(&self.root)?;
        let id = uuid::Uuid::new_v4().to_string();
        crate::durable_new(&self.root.join(&id), body.as_bytes())?;
        Ok(json!({"id":id,"sha256":hash(body),"bytes":body.len()}))
    }
    pub fn read(&self, artifact: &Value) -> Result<String> {
        let id = uuid_field(artifact, "id")?;
        crate::canonical(&self.root)?;
        let (bytes, _) = crate::repository::files::regular_bytes(&self.root, id, 4 * 1024 * 1024)
            .map_err(|_| conflict("Evidence artifact is missing or unavailable."))?;
        if artifact["bytes"].as_f64() != Some(bytes.len() as f64)
            || artifact["sha256"] != hash(&bytes)
        {
            return Err(conflict("Evidence artifact hash changed."));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
