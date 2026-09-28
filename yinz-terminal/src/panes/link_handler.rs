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
                let link = link_handler
                    .links
                    .get(link_id as usize)
                    .expect("link was not some");
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

    #[test]
    fn link_count_is_bounded_without_replacing_existing_targets() {
        let mut handler = LinkHandler::new();
        let first = handler.new_link_from_url("https://first.example".into());
        let expected = handler.output_osc8(Some(first));

        for index in 1..=MAX_LINKS + 1 {
            let anchor = if index % 2 == 0 {
                handler
                    .dispatch_osc8(&[b"8", b"", b"https://explicit.example"])
                    .unwrap()
            } else {
                handler.new_link_from_url("https://detected.example".into())
            };
            let expected_anchor = if index < MAX_LINKS {
                LinkAnchor::Start(index as u32)
            } else {
                LinkAnchor::End
            };
            assert_eq!(anchor, expected_anchor);
        }

        assert_eq!(handler.output_osc8(Some(first)), expected);
        assert_eq!(handler.links.len(), MAX_LINKS);
    }

    #[test]
    fn link_bytes_are_bounded_without_replacing_existing_targets() {
        // Both the OSC id and URI consume the same budget.
        for (params, uri) in [
            ("id=".to_owned(), "x".repeat(1024)),
            (format!("id={}", "x".repeat(1024)), "x".to_owned()),
        ] {
            let mut handler = LinkHandler::new();
            let first = handler.new_link_from_url("https://first.example".into());
            let expected = handler.output_osc8(Some(first));
            for _ in 0..100_000 {
                handler.dispatch_osc8(&[b"8", params.as_bytes(), uri.as_bytes()]);
            }
            let stored = handler.links.len();
            assert!(
                stored < MAX_LINKS,
                "the byte budget must apply before the count cap"
            );
            assert!(handler.link_bytes <= MAX_LINK_BYTES);
            assert_eq!(handler.new_link_from_url("x".repeat(1024)), LinkAnchor::End);
            assert_eq!(handler.links.len(), stored);
            assert_eq!(handler.output_osc8(Some(first)), expected);
        }
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
            let stored = link_handler
                .links
                .get(index as usize)
                .expect("link was not stored");
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
