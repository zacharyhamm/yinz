use anyhow::{anyhow, Result};
use glutin::config::Config;
use glutin::context::{
    ContextApi, ContextAttributes, ContextAttributesBuilder, GlProfile, NotCurrentContext, Version,
};
use glutin::display::Display;
use glutin::prelude::*;
use winit::raw_window_handle::RawWindowHandle;

use crate::platform::Platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextRequest {
    Gles { major: u8, minor: u8 },
    Core { major: u8, minor: u8 },
}

impl ContextRequest {
    fn attributes(self, handle: RawWindowHandle) -> ContextAttributes {
        match self {
            ContextRequest::Gles { major, minor } => ContextAttributesBuilder::new()
                .with_context_api(ContextApi::Gles(Some(Version::new(major, minor))))
                .build(Some(handle)),
            ContextRequest::Core { major, minor } => ContextAttributesBuilder::new()
                .with_context_api(ContextApi::OpenGl(Some(Version::new(major, minor))))
                .with_profile(GlProfile::Core)
                .build(Some(handle)),
        }
    }

    fn describe(self) -> String {
        match self {
            ContextRequest::Gles { major, minor } => format!("OpenGL ES {}.{}", major, minor),
            ContextRequest::Core { major, minor } => format!("OpenGL {}.{} core", major, minor),
        }
    }
}

pub fn context_attempts(platform: Platform) -> &'static [ContextRequest] {
    match platform {
        Platform::Linux => &[ContextRequest::Gles { major: 3, minor: 0 }],
        Platform::MacOs => &[
            ContextRequest::Core { major: 4, minor: 1 },
            ContextRequest::Core { major: 3, minor: 3 },
        ],
        Platform::Windows => &[
            ContextRequest::Core { major: 3, minor: 3 },
            ContextRequest::Gles { major: 3, minor: 0 },
        ],
    }
}

pub fn create_context(
    display: &Display,
    config: &Config,
    handle: RawWindowHandle,
    platform: Platform,
) -> Result<NotCurrentContext> {
    let mut refusals = Vec::new();
    for request in context_attempts(platform) {
        match unsafe { display.create_context(config, &request.attributes(handle)) } {
            Ok(context) => {
                // Every window and panel opening gets here, so only a fallback is worth saying
                // out loud; the usual choice goes to the log.
                if refusals.is_empty() {
                    log::info!("drawing through {}", request.describe());
                } else {
                    report!(
                        "drawing through {} ({})",
                        request.describe(),
                        refusals.join("; ")
                    );
                }
                return Ok(context);
            },
            Err(e) => refusals.push(format!("{}: {}", request.describe(), e)),
        }
    }
    Err(anyhow!(
        "failed to create a graphics context ({})",
        refusals.join("; ")
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderDialect {
    Gles300,
    Glsl330,
    Glsl410,
}

impl ShaderDialect {
    pub fn for_context(embedded: bool, major: u32, minor: u32) -> Result<Self> {
        let version = (major, minor);
        match (embedded, version) {
            (true, version) if version >= (3, 0) => Ok(ShaderDialect::Gles300),
            (false, version) if version >= (4, 1) => Ok(ShaderDialect::Glsl410),
            (false, version) if version >= (3, 3) => Ok(ShaderDialect::Glsl330),
            _ => Err(anyhow!(
                "the {} {}.{} context is too old: the window needs OpenGL ES 3.0 or OpenGL 3.3 core",
                if embedded { "OpenGL ES" } else { "OpenGL" },
                major,
                minor
            )),
        }
    }

    pub fn header(self) -> &'static str {
        match self {
            ShaderDialect::Gles300 => "#version 300 es\nprecision highp float;\n",
            ShaderDialect::Glsl330 => "#version 330 core\n",
            ShaderDialect::Glsl410 => "#version 410 core\n",
        }
    }

    pub fn source(self, body: &str) -> String {
        format!("{}{}", self.header(), body)
    }
}

#[cfg(test)]
#[path = "unit/graphics_tests.rs"]
mod tests;
