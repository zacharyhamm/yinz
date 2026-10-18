;; A .yinz-project directory holds identity and name UTF-8 text files.
(define marker-script "import json, pathlib, sys
cwd = pathlib.Path(sys.argv[1]).resolve()
result = None
for root in (cwd, *cwd.parents):
    marker = root / '.yinz-project'
    if marker.is_dir():
        result = dict(identity=(marker / 'identity').read_text().strip(),
                      name=(marker / 'name').read_text().strip(), root=str(root))
        break
print(json.dumps(result))")

(define (identify cwd)
  (yinz-then
    (yinz-run-command (hash "command" (list "python3" "-c" marker-script cwd)
                           "cwd" "/" "maxBytesReturned" 16384))
    (lambda (result)
      (unless (and (number? (hash-ref result "exit-code")) (= (hash-ref result "exit-code") 0)) (error (string-append "cannot read project marker (exit " (yinz-json-string (hash-ref result "exit-code")) "): " (yinz-decode-utf8 (hash-ref result "stderr")))))
      (yinz-json-parse (yinz-decode-utf8 (hash-ref result "stdout"))))))

(define (load configuration)
  (yinz-then
    (yinz-wait-for-permissions '("RunCommands" "MessageAndLaunchOtherPlugins"))
    (lambda (_)
      (yinz-project-register-provider (hash "id" "marker" "priority" 100) identify))))
(define (update event) #f)
(define (pipe message) #f)
