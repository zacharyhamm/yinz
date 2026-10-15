use anyhow::{Context, Result};
use miette::{GraphicalReportHandler, NamedSource, Report};
use serde_json::Value;
use yinz_config::config::{ConfigError, KdlError};
use yinz_types::data::LayoutParsingError;

pub(super) fn format(value: Value) -> Result<String> {
    let error = if let Value::String(source) = &value {
        if source == "SyntaxError" {
            LayoutParsingError::SyntaxError
        } else {
            match yinz_config::kdl::layout::from_kdl(
                source,
                Some("parse_layout_api".into()),
                None,
                None,
            ) {
                Ok(_) => return Ok(String::new()),
                Err(ConfigError::KdlError(error)) => LayoutParsingError::KdlError {
                    kdl_error: Box::new(error.into()),
                    file_name: "parse_layout_api".into(),
                    source_code: source.clone(),
                },
                Err(_) => LayoutParsingError::SyntaxError,
            }
        }
    } else {
        serde_json::from_value(value).context("invalid layout diagnostic")?
    };
    let output = match error {
        LayoutParsingError::KdlError { mut kdl_error, file_name, source_code } => {
            kdl_error.help_message = Some("https://github.com/zacharyhamm/yinz/tree/main/example/layouts".into());
            let report: Report = KdlError::from(*kdl_error).into();
            let report = report.with_source_code(NamedSource::new(file_name, source_code));
            let mut output = String::new();
            GraphicalReportHandler::new().render_report(&mut output, report.as_ref()).context("render layout diagnostic")?;
            output
        },
        LayoutParsingError::SyntaxError => concat!(
            "Failed to deserialize KDL node. \nPossible reasons:\n",
            "- Missing `;` after a node name, eg. { node; another_node; }\n",
            "- Missing quotations (\") around an argument node eg. { first_node \"argument_node\"; }\n",
            "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"\n",
            "- Found an extraneous equal sign (=) between node child arguments and their values. eg. { argument=\"value\" }"
        ).into(),
    };
    anyhow::ensure!(
        output.len() <= super::MAX,
        "layout diagnostic exceeds output budget"
    );
    Ok(output)
}
