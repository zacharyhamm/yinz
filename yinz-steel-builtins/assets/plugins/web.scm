;; Builtin mobile agent web app, loaded headless as the configured web plugin.
;; Builtins hold RunCommands, so load installs routes, hooks, assets and pages directly.
(define (load configuration) (yinz-web-agent-app configuration))
