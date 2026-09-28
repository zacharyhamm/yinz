# The following snippet is meant to be used like this in your fish config:
#
# if status is-interactive
#     # Configure auto-attach/exit to your likings (default is off).
#     # set YINZ_AUTO_ATTACH true
#     # set YINZ_AUTO_EXIT true
#     eval (yinz setup --generate-auto-start fish | string collect)
# end
if not set -q YINZ
    if test "$YINZ_AUTO_ATTACH" = "true"
        yinz attach -c
    else
        yinz
    end

    if test "$YINZ_AUTO_EXIT" = "true"
        kill $fish_pid
    end
end
