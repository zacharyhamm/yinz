if [[ -z "$YINZ" ]]; then
    if [[ "$YINZ_AUTO_ATTACH" == "true" ]]; then
        yinz attach -c
    else
        yinz
    fi

    if [[ "$YINZ_AUTO_EXIT" == "true" ]]; then
        exit
    fi
fi
