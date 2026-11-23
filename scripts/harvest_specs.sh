#!/bin/bash
set -e
OUT="/media/sf_Metode_Seismik_2025/rustseis/specs/raw"
mkdir -p "$OUT/menus" "$OUT/help_txt" "$OUT/prog_man_txt"

MODULES="segd_read segy_read segy_write diskread2 diskwrite geomspread_land geomspread_marine geom_load thdrmath header_values trace_len ens_define trc_edit mute debias resamp tar agc teq tvs filter fk_filt decon tvw elev_stat datum_stat_calc datum_stat_apply vaprecom71 velAnal71 nmo stack inline_sort fkmig2d kirtime2dstk traceDisplay fk_anal inter_spect"

echo "Harvesting menus and help PDFs..."
for m in $MODULES; do
    if [ -f "/apps/ProMAX/port/menu/promax/${m}.menu" ]; then
        cp "/apps/ProMAX/port/menu/promax/${m}.menu" "$OUT/menus/${m}.menu"
    fi
    if [ -f "/apps/ProMAX/port/help/promax/${m}.pdf" ]; then
        pdftotext "/apps/ProMAX/port/help/promax/${m}.pdf" "$OUT/help_txt/${m}.txt" 2>/dev/null || true
    fi
done

cp /apps/ProMAX/port/misc/header.list "$OUT/header.list"
cp /apps/ProMAX/port/menu/PromaxToolTypes.xml "$OUT/PromaxToolTypes.xml"
cp /apps/ProMAX/port/src/exe/exec/tools.db "$OUT/tools.db"

for p in exec headers opfs tables ens_define overview; do
    if [ -f "/apps/ProMAX/port/help/prog_man/${p}.pdf" ]; then
        pdftotext "/apps/ProMAX/port/help/prog_man/${p}.pdf" "$OUT/prog_man_txt/${p}.txt" 2>/dev/null || true
    fi
done

echo "DONE: menus=$(ls $OUT/menus | wc -l) help_txt=$(ls $OUT/help_txt | wc -l) prog_man=$(ls $OUT/prog_man_txt | wc -l)"
