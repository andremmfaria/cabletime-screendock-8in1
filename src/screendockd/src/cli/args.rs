//! Argument groups shared by several subcommands.

use crate::adapters::encode::EncoderKind;

use super::types::EncoderArgs;

impl EncoderArgs {
    pub(super) fn kind(&self) -> EncoderKind {
        match &self.h264_file {
            Some(path) => EncoderKind::Prerecorded(path.clone()),
            None => EncoderKind::Ffmpeg,
        }
    }
}
