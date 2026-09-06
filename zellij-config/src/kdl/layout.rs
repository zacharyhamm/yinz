use super::*;

#[allow(clippy::result_large_err)]
pub fn from_kdl(
    raw_layout: &str,
    file_name: Option<String>,
    raw_swap_layouts: Option<(&str, &str)>, // raw_swap_layouts swap_layouts_file_name
    cwd: Option<PathBuf>,
) -> Result<Layout, ConfigError> {
    let mut kdl_layout_parser = KdlLayoutParser::new(raw_layout, cwd, file_name.clone());
    let layout = kdl_layout_parser.parse().map_err(|e| match e {
        ConfigError::KdlError(kdl_error) => ConfigError::KdlError(kdl_error.add_src(
            file_name.unwrap_or_else(|| "N/A".to_owned()),
            String::from(raw_layout),
        )),
        ConfigError::KdlDeserializationError(kdl_error) => kdl_layout_error(
            kdl_error,
            file_name.unwrap_or_else(|| "N/A".to_owned()),
            raw_layout,
        ),
        e => e,
    })?;
    match raw_swap_layouts {
        Some((raw_swap_layout_filename, raw_swap_layout)) => {
            // here we use the same parser to parse the swap layout so that we can reuse assets
            // (eg. pane and tab templates)
            kdl_layout_parser
                .parse_external_swap_layouts(raw_swap_layout, layout)
                .map_err(|e| match e {
                    ConfigError::KdlError(kdl_error) => ConfigError::KdlError(kdl_error.add_src(
                        String::from(raw_swap_layout_filename),
                        String::from(raw_swap_layout),
                    )),
                    ConfigError::KdlDeserializationError(kdl_error) => kdl_layout_error(
                        kdl_error,
                        raw_swap_layout_filename.into(),
                        raw_swap_layout,
                    ),
                    e => e,
                })
        },
        None => Ok(layout),
    }
}
