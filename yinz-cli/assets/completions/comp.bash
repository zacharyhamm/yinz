function yr () { yinz run --name "$*" -- bash -ic "$*";}
function yrf () { yinz run --name "$*" --floating -- bash -ic "$*";}
function yri () { yinz run --name "$*" --in-place -- bash -ic "$*";}
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