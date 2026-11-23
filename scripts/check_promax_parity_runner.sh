#!/bin/bash
export PROMAX_HOME=/apps/ProMAX
export PROMAX_DATA_HOME=/media/sf_Mrk_01/
export PROWESS_HOME=/apps/SeisSpace
export PROWESS_HOST=localhost
export PROWESS_PORT=5006
export LD_LIBRARY_PATH=/apps/ProMAX/linux/lib:/apps/ProMAX/sys/lib:/apps/ProMAX/jre/lib/i386/server:/usr/lib:/lib
export PATH=/apps/ProMAX/sys/bin:/apps/ProMAX/port/bin:/apps/ProMAX/sys/exe:/apps/SeisSpace/port/bin:$PATH

echo "=== 1. ProMAX Raw_Shot_1000 binary check ==="
mkdir -p /media/sf_Metode_Seismik_2025/rustseis/out/promax_parity
cp /media/sf_Mrk_01/test_seismik/line_01/42597984/TRC1 /media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_raw_shot1000_TRC1.bin
cp /media/sf_Mrk_01/test_seismik/line_01/42597984/HDR1 /media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_raw_shot1000_HDR1.bin
ls -lh /media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/

echo "=== 2. Check SeisSpace submitFlow / pw.sh ==="
ls -la /apps/SeisSpace/port/bin/pw.sh /apps/SeisSpace/port/bin/submitFlow
head -n 35 /apps/SeisSpace/port/bin/pw.sh

echo "=== 3. Check if ProMAX has standalone SEGY/Filter/AGC test ==="
/apps/ProMAX/linux/bin/proloop -h 2>&1 | head -n 15
echo "=== 4. Check existing SeisSpace test flows for segy/filter/decon/nmo ==="
ls /apps/SeisSpace/test/promax/data/testarea/testflowsline/ | head -n 40
