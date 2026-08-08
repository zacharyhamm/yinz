use std::collections::HashMap;

use super::LinkAnchor;

const TERMINATOR: &str = "\u{1b}\\";

#[derive(Debug, Clone)]
pub struct LinkHandler {
    links: HashMap<u16, Link>,
    link_index: u16,
}
#[derive(Debug, Clone)]
pub struct Link {
    pub id: Option<String>,
    pub uri: String,
}

impl LinkHandler {
    pub fn new() -> Self {
        Self {
            links: HashMap::new(),
            link_index: 0,
        }
    }

    pub fn dispatch_osc8(&mut self, params: &[&[u8]]) -> Option<LinkAnchor> {
        let (link_params, uri) = (params[1], params[2]);
        log::debug!(
            "dispatching osc8, params: {:?}, uri: {:?}",
            std::str::from_utf8(link_params),
            std::str::from_utf8(uri)
        );

        if !uri.is_empty() {
            // save the link, and the id if present to hashmap
            String::from_utf8(uri.to_vec()).ok().map(|uri| {
                let id = link_params
                    .split(|&b| b == b':')
                    .find(|kv| kv.starts_with(b"id="))
                    .and_then(|kv| String::from_utf8(kv[3..].to_vec()).ok());
                let anchor = LinkAnchor::Start(self.link_index);
                self.links.insert(self.link_index, Link { id, uri });
                self.link_index += 1;
                anchor
            })
        } else {
            // there is no link, so consider it a link end
            Some(LinkAnchor::End)
        }
    }

    pub fn new_link_from_url(&mut self, url: String) -> LinkAnchor {
        let anchor = LinkAnchor::Start(self.link_index);
        self.links.insert(
            self.link_index,
            Link {
                id: Some(self.link_index.to_string()),
                uri: url,
            },
        );
        self.link_index += 1;
        anchor
    }

    pub fn output_osc8(&self, link_anchor: Option<LinkAnchor>) -> Option<String> {
        link_anchor.and_then(|link| match link {
            LinkAnchor::Start(index) => {
                let link = self.links.get(&index);

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
    pub fn links(&self) -> HashMap<u16, Link> {
        self.links.clone()
    }
}

impl Default for LinkHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_osc8_link_start() {
        let mut link_handler = LinkHandler::default();
        let link_params = "id=test";
        let uri = "http://test.com";
        let params = vec!["8".as_bytes(), link_params.as_bytes(), uri.as_bytes()];

        let anchor = link_handler.dispatch_osc8(&params);

        match anchor {
            Some(LinkAnchor::Start(link_id)) => {
                let link = link_handler.links.get(&link_id).expect("link was not some");
                assert_eq!(link.id, Some("test".to_string()));
                assert_eq!(link.uri, uri);
            },
            _ => panic!("pending link handler was not start"),
        }

        let expected = format!("\u{1b}]8;id=test;http://test.com{}", TERMINATOR);
        assert_eq!(link_handler.output_osc8(anchor).unwrap(), expected);
    }

    #[test]
    fn dispatch_osc8_link_end() {
        let mut link_handler = LinkHandler::default();
        let params: Vec<&[_]> = vec![b"8", b"", b""];

        let anchor = link_handler.dispatch_osc8(&params);

        assert_eq!(anchor, Some(LinkAnchor::End));

        let expected = format!("\u{1b}]8;;{}", TERMINATOR);
        assert_eq!(link_handler.output_osc8(anchor).unwrap(), expected);
    }

    #[test]
    fn return_none_on_missing_link_id() {
        let link_handler = LinkHandler::default();
        let anchor = LinkAnchor::Start(100);
        assert_eq!(link_handler.output_osc8(Some(anchor)), None);
    }

    mod props {
        use super::*;
        use hegel::generators::{self, Generator};

        /// Generalizes `dispatch_osc8_link_start`, which round-tripped one
        /// hardcoded id and uri. Anything a program emits must come back out
        /// byte-identical, or hyperlinks break for that program.
        #[hegel::test]
        fn a_dispatched_link_is_re_emitted_verbatim(tc: hegel::TestCase) {
            // OSC payloads are delimited by `;` and `:`, and the terminator
            // ends the sequence - a uri containing them is a different test.
            let uri = tc
                .draw(generators::text().min_size(1))
                .replace(['\u{1b}', ';', ':'], "");
            let id = tc
                .draw(generators::text())
                .replace(['\u{1b}', ';', ':'], "");
            tc.assume(!uri.is_empty());

            let mut link_handler = LinkHandler::default();
            let params = vec![
                "8".as_bytes(),
                format!("id={}", id).into_bytes().leak() as &[u8],
                uri.as_bytes(),
            ];
            let anchor = link_handler.dispatch_osc8(&params);

            let Some(LinkAnchor::Start(index)) = anchor else {
                panic!(
                    "dispatching uri {:?} did not start a link: {:?}",
                    uri, anchor
                );
            };
            let stored = link_handler.links.get(&index).expect("link was not stored");
            assert_eq!(
                stored.uri, uri,
                "stored uri differs from the dispatched one"
            );
            assert_eq!(stored.id.as_deref(), Some(id.as_str()), "stored id differs");

            let expected = format!("\u{1b}]8;id={};{}{}", id, uri, TERMINATOR);
            assert_eq!(
                link_handler.output_osc8(anchor),
                Some(expected),
                "re-emitting the link for uri {:?} id {:?} changed it",
                uri,
                id
            );
        }

        /// Two links must never collide on one index, or the second overwrites
        /// the first and every cell anchored to it re-points.
        #[hegel::test]
        fn every_dispatched_link_gets_its_own_index(tc: hegel::TestCase) {
            let uris = tc.draw(
                generators::vecs(
                    generators::text()
                        .min_size(1)
                        .max_size(20)
                        .map(|s| s.replace(['\u{1b}', ';', ':'], "")),
                )
                .min_size(1)
                .max_size(30),
            );
            let mut link_handler = LinkHandler::default();
            let mut indices = std::collections::HashSet::new();
            let mut dispatched = 0;
            for uri in &uris {
                if uri.is_empty() {
                    continue;
                }
                if let Some(LinkAnchor::Start(index)) =
                    link_handler.dispatch_osc8(&["8".as_bytes(), b"", uri.as_bytes()])
                {
                    assert!(indices.insert(index), "index {} was reused", index);
                    dispatched += 1;
                }
            }
            assert_eq!(
                link_handler.links().len(),
                dispatched,
                "{} links dispatched but {} stored",
                dispatched,
                link_handler.links().len()
            );
        }

        /// OSC payloads come straight off the pty. `dispatch_osc8` indexes
        /// `params[1]` and `params[2]` without checking the length, so a short
        /// or malformed OSC 8 is a robustness question, not a formatting one.
        #[hegel::test]
        fn dispatching_arbitrary_osc_params_never_panics(tc: hegel::TestCase) {
            let params: Vec<Vec<u8>> = tc.draw(generators::vecs(generators::binary()).max_size(5));
            let borrowed: Vec<&[u8]> = params.iter().map(|p| p.as_slice()).collect();
            let mut link_handler = LinkHandler::default();
            let anchor = link_handler.dispatch_osc8(&borrowed);
            // Whatever it decides, re-emitting must not panic either.
            let _ = link_handler.output_osc8(anchor);
        }
    }
}
