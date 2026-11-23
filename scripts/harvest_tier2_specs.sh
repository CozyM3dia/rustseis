#!/bin/bash
set -e
OUT="/media/sf_Metode_Seismik_2025/rustseis/specs/raw_tier2"
mkdir -p "$OUT/menus" "$OUT/help_txt"

echo "Listing matching Tier 2 menus..."
ls /apps/ProMAX/port/menu/promax/*.menu | grep -iE '(fxdecon|radon|taup|lfaf|airblast|srme|asub|scamp|scdecon|fbpick|refstat|autostat|pwr_stat|trimstat|dmo|kirtime|fdmig|miget|synth|afd|crook|vsp)' | sed 's|.*/||; s|\.menu$||' | sort -u > "$OUT/matched_modules.txt"

while read -r m; do
    if [ -f "/apps/ProMAX/port/menu/promax/${m}.menu" ]; then
        cp "/apps/ProMAX/port/menu/promax/${m}.menu" "$OUT/menus/${m}.menu"
    fi
    if [ -f "/apps/ProMAX/port/help/promax/${m}.pdf" ]; then
        pdftotext "/apps/ProMAX/port/help/promax/${m}.pdf" "$OUT/help_txt/${m}.txt" 2>/dev/null || true
    fi
done < "$OUT/matched_modules.txt"

echo "DONE_TIER2: matched=$(wc -l < "$OUT/matched_modules.txt") menus=$(ls "$OUT/menus" | wc -l) help_txt=$(ls "$OUT/help_txt" | wc -l)"
cat "$OUT/matched_modules.txt"
