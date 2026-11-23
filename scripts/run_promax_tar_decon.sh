#!/bin/bash
set -e
export PROMAX_HOME=/apps/ProMAX
export PROWESS_HOME=/apps/SeisSpace
IN_SGY="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/input_nias_shot1000.sgy"
OUT_TAR="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_tar.sgy"
OUT_DECON="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_decon.sgy"

mkdir -p /tmp/parity_tar /tmp/parity_decon
cat > /tmp/parity_tar/exec.0.pwflow << EOF
<parset name="parity_tar">
  <par name="flowType" type="string"> PWFlow </par>
  <parset name="flowParameters"><parset name="etc"><par name="show" type="boolean"> true </par></parset></parset>
  <parset name="modules">
    <parset name="com.lgc.prowess.tool.segyinput.SegYInputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyinput.SegYInputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="multiple_files" type="boolean"> false </par>
        <par name="single_file_PATH" type="string"> $IN_SGY </par>
        <par name="FILEPATH" type="string"> $IN_SGY </par>
        <par name="TRACE_FORMAT" type="string"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
        <par name="PRIMARY_SORT" type="string"> FFID </par>
        <par name="ENSEMBLE_BREAK_HDR" type="string"> FFID </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.tar.TarTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.tar.TarTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="APPLY_SD" type="int"> 0 </par>
        <par name="SDV_TYPE" type="int"> 1 </par>
        <par name="APPLY_IE" type="int"> 0 </par>
        <par name="APPLY_DB" type="int"> 1 </par>
        <par name="TAR_REF" type="int"> 0 </par>
        <par name="DB_PER_S" type="float"> 6.0 </par>
        <par name="APPLY_PW" type="int"> 0 </par>
        <par name="T_POWER" type="float"> 2.0 </par>
        <par name="TAR_MODE" type="int"> 1 </par>
        <par name="TIME_MAX" type="float"> 0.0 </par>
        <par name="NORM_SRC" type="int"> 2 </par>
        <par name="REF_TIME" type="float"> 0.0 </par>
        <par name="TAR_NORM" type="float"> 1.0 </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_TAR </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
    </parset>
  </parset>
  <parset name="connections">
    <parset name="connection #1">
      <par name="src" type="string"> "com.lgc.prowess.tool.segyinput.SegYInputTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.tar.TarTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
    <parset name="connection #2">
      <par name="src" type="string"> "com.lgc.prowess.tool.tar.TarTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.segyoutput.SegYOutputTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
  </parset>
</parset>
EOF

cat > /tmp/parity_decon/exec.0.pwflow << EOF
<parset name="parity_decon">
  <par name="flowType" type="string"> PWFlow </par>
  <parset name="flowParameters"><parset name="etc"><par name="show" type="boolean"> true </par></parset></parset>
  <parset name="modules">
    <parset name="com.lgc.prowess.tool.segyinput.SegYInputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyinput.SegYInputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="multiple_files" type="boolean"> false </par>
        <par name="single_file_PATH" type="string"> $IN_SGY </par>
        <par name="FILEPATH" type="string"> $IN_SGY </par>
        <par name="TRACE_FORMAT" type="string"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
        <par name="PRIMARY_SORT" type="string"> FFID </par>
        <par name="ENSEMBLE_BREAK_HDR" type="string"> FFID </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.decon.DeconTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.decon.DeconTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="DCN_TYPE" type="int"> 1 </par>
        <par name="DCN_OPER" type="string"> 160.0 </par>
        <par name="DCN_PRED" type="string"> 16.0 </par>
        <par name="wb_option" type="int"> 0 </par>
        <par name="pred_correction" type="int"> 0 </par>
        <par name="RATIO" type="float"> 0.0 </par>
        <par name="TAPEROPT" type="int"> 1 </par>
        <par name="TAPERLEN" type="float"> 0.0 </par>
        <par name="DCN_WHTN" type="string"> 0.1 </par>
        <par name="TWIN_MIN" type="float"> 2.0 </par>
        <par name="GATETIME" type="int"> 1 </par>
        <par name="db_option" type="int"> 0 </par>
        <par name="PRIM_KEY" type="string"> FFID </par>
        <par name="SCND_KEY" type="string"> AOFFSET </par>
        <par name="DCN_GATE" type="string"> 1000:0:0-4000/ </par>
        <par name="IOUT_OPT" type="int"> 1 </par>
        <par name="filter_option" type="int"> 0 </par>
        <par name="remute_option" type="int"> 0 </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_DECON </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
    </parset>
  </parset>
  <parset name="connections">
    <parset name="connection #1">
      <par name="src" type="string"> "com.lgc.prowess.tool.segyinput.SegYInputTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.decon.DeconTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
    <parset name="connection #2">
      <par name="src" type="string"> "com.lgc.prowess.tool.decon.DeconTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.segyoutput.SegYOutputTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
  </parset>
</parset>
EOF

/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_tar/exec.0.pwflow | tail -n 6 || true
/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_decon/exec.0.pwflow | tail -n 6 || true
ls -lh "$OUT_TAR" "$OUT_DECON" 2>&1 || true
