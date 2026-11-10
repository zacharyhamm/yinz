;; Keep eventual app content behind this explicit installation boundary.
;; Standard app routes/assets are delivered separately; initialization has no commands.
(define %web-app-installed #f)
(provide %web-app-install)
(define (%web-app-install configuration)
  (unless %web-app-installed (error "standard web agent app is not installed"))
  json-null)
