# Fixed SDK scalar wire. Caller first proves regular private file, size and final LF.
# Exit 0 permits one STOP only; exit 1 is no qualifying record; exit 90 is invalid.
function positive(s) {
    return s ~ /^[1-9][0-9]*$/ && (length(s) < 20 ||
           (length(s) == 20 && "x" s <= "x18446744073709551615"))
}
function natural(s) { return s == "0" || positive(s) }
function event(s) {
    return s == "startup" || s == "window" || s == "before-render" ||
           s == "after-render" || s == "about-to-quit" ||
           s == "window-destroyed" || s == "application-destroyed" ||
           s == "late-before-render" || s == "late-after-render"
}
BEGIN { valid = positive(pid) && positive(start); before = 0; previous_ns = 0 }
{
    if (NR > 96 || length($0) >= 192 || $0 ~ /[^a-z0-9 -]/ ||
        NF != 10 || $0 != $1 " " $2 " " $3 " " $4 " " $5 " " $6 " " $7 " " $8 " " $9 " " $10 ||
        $1 != "v1" || !positive($2) || $2 != NR || !event($3) ||
        !positive($4) || !positive($5) || !positive($6) || !positive($7) ||
        "x" $6 != "x" pid || "x" $7 != "x" start ||
        ($8 != "0" && $8 != "1") || $9 != "0" || !natural($10) ||
        $4 + 0 < previous_ns || (NR == 1 && $3 != "startup")) valid = 0
    previous_ns = $4 + 0
    if ($3 == "before-render" && $10 + 0 >= 1) before = 1
}
END { if (!valid || NR == 0) exit 90; if (!before) exit 1; exit 0 }
