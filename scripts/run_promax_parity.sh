#!/bin/bash
set -e
export PROMAX_HOME=/apps/ProMAX
export PROWESS_HOME=/apps/SeisSpace
export PROWESS_HOST=localhost
export PROWESS_PORT=5006
export LD_LIBRARY_PATH=/apps/ProMAX/linux/lib:/apps/ProMAX/sys/lib:/apps/ProMAX/jre/lib/i386/server:/usr/lib:/lib
export PATH=/apps/ProMAX/sys/bin:/apps/ProMAX/port/bin:/apps/ProMAX/sys/exe:/apps/SeisSpace/port/bin:$PATH

IN_SGY="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/input_nias_shot1000.sgy"
OUT_BP="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_bp.sgy"
OUT_AGC="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_agc.sgy"

mkdir -p /tmp/parity_bp /tmp/parity_agc
cat > /tmp/parity_bp/exec.0.pwflow << EOF
<parset name="parity_bp">
  <par name="flowType" type="string"> PWFlow </par>
  <parset name="flowParameters">
    <parset name="etc"><par name="show" type="boolean"> true </par></parset>
    <parset name="Submit">
      <par name="nExecsPerNode" type="int"> 1 </par>
      <par name="Hosts" type="string"> localhost </par>
      <par name="nJobletsPerExec" type="int"> 1 </par>
      <par name="submitTo" type="string"> Hosts </par>
    </parset>
    <parset name="EXEC">
      <parset name="PROCDESC">
        <par name="product" type="string"> PROWESS </par>
        <par name="name" type="string"> com.lgc.prodesk.flowbuilder.menu.ExecutiveProc </par>
        <par name="type" type="string"> Java </par>
      </parset>
      <parset name="menuParameters">
        <par name="RECOMENDED_NUM_JOBLETS" type="int"> 1 </par>
        <par name="JAVA_XMX_VALUE" type="long"> 512 </par>
      </parset>
    </parset>
  </parset>
  <parset name="modules">
    <parset name="com.lgc.prowess.tool.segyinput.SegYInputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyinput.SegYInputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters">
        <parset name="menuParameters">
          <par name="multiple_files" type="boolean"> false </par>
          <par name="single_file_PATH" type="string"> $IN_SGY </par>
          <par name="FILEPATH" type="string"> $IN_SGY </par>
          <par name="PRINT_FILEINFO" type="boolean"> true </par>
          <par name="OVERRIDE_HEADER" type="boolean"> false </par>
          <par name="TRACE_FORMAT" type="string"> 5 </par>
          <par name="BYTESWAP" type="boolean"> false </par>
          <par name="PRIMARY_SORT" type="string"> FFID </par>
          <par name="ENSEMBLE_BREAK_HDR" type="string"> FFID </par>
        </parset>
      </parset>
    </parset>
    <parset name="com.lgc.prowess.tool.filter.FilterTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.filter.FilterTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters">
        <parset name="menuParameters">
          <par name="FLT_TYPE" type="int"> 1 </par>
          <par name="FLT_SPEC" type="int"> 1 </par>
          <par name="PHASE" type="int"> 1 </par>
          <par name="RNOISE" type="float"> 1.0 </par>
          <par name="IDOMAIN" type="int"> 2 </par>
          <par name="FILT_LEN" type="float"> 160.0 </par>
          <par name="PCT_PAD" type="float"> 25.0 </par>
          <par name="NOTCH_OPTION" type="boolean"> false </par>
          <par name="FLT_FREQ1" type="string"> 10-20-60-80 </par>
          <par name="REMUTE_OPTION" type="boolean"> false </par>
        </parset>
      </parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters">
        <parset name="menuParameters">
          <par name="FILEPATH" type="string"> $OUT_BP </par>
          <par name="USE_HDR_IN_PATH" type="boolean"> false </par>
          <par name="FORMAT_CODE" type="int"> 5 </par>
          <par name="BYTESWAP" type="boolean"> false </par>
        </parset>
      </parset>
    </parset>
  </parset>
  <parset name="connections">
    <parset name="connection #1">
      <par name="src" type="string"> "com.lgc.prowess.tool.segyinput.SegYInputTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.filter.FilterTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
    <parset name="connection #2">
      <par name="src" type="string"> "com.lgc.prowess.tool.filter.FilterTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.segyoutput.SegYOutputTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
  </parset>
</parset>
EOF

echo "Running SeisSpace submitFlow for Bandpass parity..."
/apps/SeisSpace/port/bin/submitFlow /tmp/parity_bp localhost 1 1 2>&1 || true
ls -lh "$OUT_BP" 2>&1 || true
