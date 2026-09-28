function __yinz_complete_sessions
    yinz list-sessions --short --no-formatting 2>/dev/null
end
complete -c yinz -n "__fish_seen_subcommand_from attach" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from a" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from kill-session" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from k" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from delete-session" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from d" -f -a "(__yinz_complete_sessions)" -d Session
complete -c yinz -n "__fish_seen_subcommand_from setup" -l generate-completion -x -a "bash elvish fish zsh" -d Shell
function yzr
  command yinz run --name "$argv" -- fish -c "$argv"
end
function yzrf
  command yinz run --name "$argv" --floating -- fish -c "$argv"
end
function yzri
  command yinz run --name "$argv" --in-place -- fish -c "$argv"
end
function yze
  command yinz edit $argv
end
function yzef
  command yinz edit --floating $argv
end
function yzei
  command yinz edit --in-place $argv
end

# the yzpipe alias and its completions
function __yinz_complete_aliases
  yinz list-aliases 2>/dev/null
end
function yzpipe
  if count $argv > /dev/null
    command yinz pipe -p $argv
  else
    command yinz pipe
  end
end
complete -c yzpipe -f -a "(__yinz_complete_aliases)" -d Yzpipes
