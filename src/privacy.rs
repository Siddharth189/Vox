use crate::error::Result;
use crate::model::{AppContext, Privacy};

#[derive(Debug, Default, Clone)]
pub struct PrivacyPolicy;

impl PrivacyPolicy {
    pub fn ensure_allowed(&self, ctx: &AppContext) -> Result<()> {
        if ctx.profile.privacy == Privacy::Disabled {
            return Err(crate::error::VoxError::PrivacyBlocked(format!(
                "{} is a disabled app; Vox will not listen or inject here",
                ctx.app_name
            )));
        }
        Ok(())
    }
}
