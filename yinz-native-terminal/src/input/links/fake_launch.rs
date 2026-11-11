thread_local! {
    static FAIL_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static OPENED: std::cell::RefCell<Vec<(String, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn launch(opener: &str, uri: &str) -> Result<(), String> {
    if FAIL_NEXT.with(|fail| fail.replace(false)) {
        return Err("simulated launcher failure".into());
    }
    OPENED.with(|opened| {
        opened
            .borrow_mut()
            .push((opener.to_owned(), uri.to_owned()))
    });
    Ok(())
}

pub fn opened() -> Vec<(String, String)> {
    OPENED.with(|opened| opened.borrow().clone())
}

pub fn forget_opened() {
    OPENED.with(|opened| opened.borrow_mut().clear());
}

pub fn fail_next() {
    FAIL_NEXT.with(|fail| fail.set(true));
}
