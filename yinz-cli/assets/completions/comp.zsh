function yr () { yinz run --name "$*" -- zsh -ic "$*";}
function yrf () { yinz run --name "$*" --floating -- zsh -ic "$*";}
function yri () { yinz run --name "$*" --in-place -- zsh -ic "$*";}
function ye () { yinz edit "$*";}
function yef () { yinz edit --floating "$*";}
function yei () { yinz edit --in-place "$*";}
function ypipe () { 
  if [ -z "$1" ]; then
    yinz pipe;
  else 
    yinz pipe -p $1;
  fi
}