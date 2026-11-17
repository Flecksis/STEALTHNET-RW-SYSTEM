use crate::module::Module;
use async_trait::async_trait;
use sn_domain::module::ModuleId;

pub struct Placeholder(pub ModuleId);

#[async_trait]
impl Module for Placeholder {
    fn id(&self) -> ModuleId {
        self.0
    }
}
