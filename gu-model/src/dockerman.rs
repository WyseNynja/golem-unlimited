use serde_derive::*;

#[derive(Default, Serialize, Deserialize)]
pub struct CreateOptions {
    pub volumes: Vec<VolumeDef>,
    pub cmd: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Hash, Clone, Eq, PartialEq)]
pub enum VolumeDef {
    BindRw { src: String, target: String },
}

impl VolumeDef {
    pub fn source_dir(&self) -> Option<&String> {
        match self {
            VolumeDef::BindRw { src, target } => Some(src),
        }
    }

    pub fn target_dir(&self) -> Option<&String> {
        match self {
            VolumeDef::BindRw { src, target } => Some(target),
        }
    }
}
