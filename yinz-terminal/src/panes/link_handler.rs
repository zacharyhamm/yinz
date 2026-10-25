use super::LinkAnchor;

const TERMINATOR: &str = "\u{1b}\\";
const MAX_LINKS: usize = 64 * 1024;
const MAX_LINK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LinkHandler {
    links: Vec<Link>,
    link_bytes: usize,
}
#[derive(Debug, Clone)]
pub struct Link {
    pub id: Option<String>,
    pub uri: String,
}

impl LinkHandler {
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            link_bytes: 0,
        }
    }

    pub fn dispatch_osc8(&mut self, params: &[&[u8]]) -> Option<LinkAnchor> {
        let (Some(&link_params), Some(&uri)) = (params.get(1), params.get(2)) else {
            return None;
        };
        log::debug!(
            "dispatching osc8, params: {:?}, uri: {:?}",
            std::str::from_utf8(link_params),
            std::str::from_utf8(uri)
        );

        if !uri.is_empty() {
            // Retain a stable index while any scrollback cell can reference the link.
            String::from_utf8(uri.to_vec()).ok().map(|uri| {
                let id = link_params
                    .split(|&b| b == b':')
                    .find(|kv| kv.starts_with(b"id="))
                    .and_then(|kv| String::from_utf8(kv[3..].to_vec()).ok());
                self.insert_link(Link { id, uri })
            })
        } else {
            // there is no link, so consider it a link end
            Some(LinkAnchor::End)
        }
    }

    pub fn new_link_from_url(&mut self, url: String) -> LinkAnchor {
        self.insert_link(Link {
            id: Some(self.links.len().to_string()),
            uri: url,
        })
    }

    fn insert_link(&mut self, link: Link) -> LinkAnchor {
        let link_bytes = link.uri.capacity() + link.id.as_ref().map_or(0, String::capacity);
        // ponytail: stop adding links at either cap; reclaim through cell ownership if needed.
        // Never reuse an index: another pane or a scrollback cell may still reference it.
        if self.links.len() >= MAX_LINKS || link_bytes > MAX_LINK_BYTES - self.link_bytes {
            return LinkAnchor::End;
        }
        let index = self.links.len() as u32;
        self.link_bytes += link_bytes;
        self.links.push(link);
        LinkAnchor::Start(index)
    }

    pub fn output_osc8(&self, link_anchor: Option<LinkAnchor>) -> Option<String> {
        link_anchor.and_then(|link| match link {
            LinkAnchor::Start(index) => {
                let link = self.links.get(index as usize);

                let output = link.map(|link| {
                    let id = link
                        .id
                        .as_ref()
                        .map_or("".to_string(), |id| format!("id={}", id));
                    format!("\u{1b}]8;{};{}{}", id, link.uri, TERMINATOR)
                });

                if output.is_none() {
                    log::warn!(
                        "attempted to output osc8 link start, but id: {} was not found!",
                        index
                    );
                }

                output
            },
            LinkAnchor::End => Some(format!("\u{1b}]8;;{}", TERMINATOR)),
        })
    }

    #[cfg(test)]
    pub fn links(&self) -> &[Link] {
        &self.links
    }
}

impl Default for LinkHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "unit/link_handler_tests.rs"]
mod tests;
