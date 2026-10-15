(require "./common.scm")
(provide ff-script ff-structured-script ff-selection ff-validate-options)

(define (ff-script config out)
  (define custom (trim (ff-get config "file_command" "")))
  (define args (trim (ff-get config "fzf_args" "")))
  (define git "git ls-files --cached --others --exclude-standard -z | tr '\\000' '\\n'")
  (define find "find . -type f -not -path '*/.git/*' | sed 's|^\\./||'")
  (define start "if root=$(git rev-parse --show-toplevel 2>/dev/null) && [ -n \"$root\" ]; then\n  cd \"$root\" || exit 1\n")
  (string-append "set -u\n"
    (if (equal? custom "")
      (string-append start "  list() { " git "; }\nelse\n  root=$PWD\n  list() { " find "; }\nfi\n")
      (string-append start "else\n  root=$PWD\nfi\nlist() { " custom "; }\n"))
    "sel=$(list | fzf --prompt 'file> '" (if (equal? args "") "" (string-append " " args)) ") || exit 0\n"
    "[ -n \"$sel\" ] || exit 0\ncase \"$sel\" in\n"
    "  /*) printf '%s\\n' \"$sel\" ;;\n  *)  printf '%s/%s\\n' \"$root\" \"$sel\" ;;\nesac > " (ff-quote out) "\n"))

(define (ff-validate-options options)
  (ff-fields options '("kind" "roots" "extensions" "hidden" "recursive" "timeoutMs"))
  (define kind (ff-get options "kind" #f))
  (unless (member kind '("file" "directory")) (error "kind must be file or directory"))
  (define roots (ff-get options "roots" json-null))
  (when (ff-present? roots)
    (ff-strings roots "roots")
    (when (or (null? roots) (ff-any (lambda (root) (or (equal? root "") (string-contains? root "\u{0}"))) roots))
      (error "roots must contain at least one nonempty path without NUL")))
  (define extensions (ff-get options "extensions" json-null))
  (when (ff-present? extensions)
    (ff-strings extensions "extensions")
    (when (equal? kind "directory") (error "extensions are only supported for file picks"))
    (when (ff-any (lambda (ext)
          (or (equal? ext "") (starts-with? ext ".")
            (ff-any (lambda (bad) (string-contains? ext bad)) '("\u{0}" "/" "\\" "*" "?" "[" "]")))) extensions)
      (error "extensions must be literal suffixes without leading dots or glob syntax")))
  (for-each (lambda (name)
      (define value (ff-get options name json-null))
      (when (and (ff-present? value) (not (boolean? value))) (error (string-append name " must be boolean")))) '("hidden" "recursive"))
  (define timeout (ff-get options "timeoutMs" json-null))
  (when (and (ff-present? timeout) (not (and (integer? timeout) (>= timeout 0)))) (error "timeoutMs must be unsigned"))
  options)

(define (ff-structured-script config options cwd out)
  (ff-validate-options options)
  (define roots (ff-get options "roots" json-null))
  (define extensions (ff-get options "extensions" json-null))
  (define hidden (if (equal? (ff-get options "hidden" #f) #t) "" "\\( ! -path \"$root_pattern\" -name '.*' \\) -prune -o "))
  (define recursive (not (equal? (ff-get options "recursive" #t) #f)))
  (define kind (if (equal? (hash-ref options "kind") "directory") "d" "f"))
  (define suffixes
    (if (or (not (ff-present? extensions)) (null? extensions)) ""
      (string-append "\\( " (string-join (map (lambda (extension) (string-append "-iname " (ff-quote (string-append "*." extension)))) extensions) " -o ") " \\) ")))
  (define select (string-append "-type " kind " ! -samefile \"$out\" ! -samefile \"$candidates\" " suffixes "-print0"))
  (define expression (if recursive select (string-append "\\( -path \"$root_pattern\" -o -prune \\) -a \\( " select " \\)")))
  (define listings
    (map (lambda (root)
        (string-append "(cd -P " (ff-quote (ff-join cwd root))
          " && root=$PWD && root_pattern=$(printf '%s/' \"$root\" | sed 's/[][\\\\*?]/\\\\&/g') && root_pattern=${root_pattern%/} && find \"$root\" " hidden expression ")"))
      (if (ff-present? roots) roots (list cwd))))
  (string-append
    "set -u\nout=" (ff-quote out) "\ncandidates=" (ff-quote (string-append out ".candidates")) "\n"
    "trap 'rm -f \"$candidates\"' EXIT\ntrap 'exit 129' HUP\ntrap 'exit 130' INT\ntrap 'exit 143' TERM\n"
    ": > \"$out\" || exit 10\ncommand -v fzf >/dev/null 2>&1 || exit 11\n{ "
    (string-join listings " &&\n") "; } > \"$candidates\" || exit 10\n"
    "fzf " (ff-get config "fzf_args" "") " --no-multi --read0 --print0 --no-print-query --no-expect < \"$candidates\" > \"$out\"\n"
    "status=$?\ncase \"$status\" in\n  0) exit 0 ;;\n  1|130) : > \"$out\"; exit 0 ;;\n  *) : > \"$out\"; exit 12 ;;\nesac\n"))

(define (ff-selection contents structured)
  (cond
    [(equal? contents "") json-null]
    [structured
      (unless (ends-with? contents "\u{0}") (error "finder output is not NUL-terminated"))
      (define selection (substring contents 0 (- (string-length contents) 1)))
      (when (or (string-contains? selection "\u{0}") (not (ff-absolute? selection)))
        (error "finder must return exactly one absolute path")) selection]
    [else (define first (car (split-many contents "\n"))) (if (equal? first "") json-null first)]))
