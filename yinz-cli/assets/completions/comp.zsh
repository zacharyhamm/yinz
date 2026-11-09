function yzr () { yinz run --name "$*" -- zsh -ic "$*";}
function yzrf () { yinz run --name "$*" --floating -- zsh -ic "$*";}
function yzri () { yinz run --name "$*" --in-place -- zsh -ic "$*";}
function yze () { yinz edit "$*";}
function yzef () { yinz edit --floating "$*";}
function yzei () { yinz edit --in-place "$*";}
function yzpipe () { 
  if [ -z "$1" ]; then
    yinz pipe;
  else 
    yinz pipe -p $1;
  fi
}
# set-password --password-stdin / --clear / --revoke-sessions are generated
# from the CLI schema above. Password values are never arguments or completions.

# web status --https-port is generated from the CLI schema.
