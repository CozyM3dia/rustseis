#!/bin/bash
set -e
export PROMAX_HOME=/apps/ProMAX
export PROWESS_HOME=/apps/SeisSpace
IN_SGY="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/input_nias_shot1000.sgy"
OUT_AGC="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_agc.sgy"
OUT_BUTT="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_butt_zero.sgy"
OUT_MINP="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_ormsby_minp.sgy"

mkdir -p /tmp/parity_agc /tmp/parity_butt /tmp/parity_minp

cat > /tmp/parity_agc/exec.0.pwflow << EOF
<parset name="parity_agc">
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
    <parset name="com.lgc.prowess.tool.agc.AgcTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.agc.AgcTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="MODE_APP" type="string"> 1 </par>
        <par name="MODE_AMP" type="string"> 2 </par>
        <par name="AGCLEN" type="float"> 500.0 </par>
        <par name="MODE_WIN" type="string"> 2 </par>
        <par name="MODEZERO" type="int"> 0 </par>
        <par name="ROBUST_OPTION" type="boolean"> false </par>
        <par name="ROBUST" type="float"> 20.0 </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_AGC </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
    </parset>
  </parset>
  <parset name="connections">
    <parset name="connection #1">
      <par name="src" type="string"> "com.lgc.prowess.tool.segyinput.SegYInputTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.agc.AgcTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
    <parset name="connection #2">
      <par name="src" type="string"> "com.lgc.prowess.tool.agc.AgcTool #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.segyoutput.SegYOutputTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
  </parset>
</parset>
EOF

cat > /tmp/parity_butt/exec.0.pwflow << EOF
<parset name="parity_butt">
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
    <parset name="com.lgc.prowess.tool.filter.FilterTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.filter.FilterTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FLT_TYPE" type="int"> 1 </par>
        <par name="FLT_SPEC" type="int"> 2 </par>
        <par name="PHASE" type="int"> 1 </par>
        <par name="RNOISE" type="float"> 1.0 </par>
        <par name="IDOMAIN" type="int"> 2 </par>
        <par name="PCT_PAD" type="float"> 25.0 </par>
        <par name="NOTCH_OPTION" type="boolean"> false </par>
        <par name="FLT_FREQ2" type="string"> 15-18-60-24 </par>
        <par name="REMUTE_OPTION" type="boolean"> false </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_BUTT </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
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

cat > /tmp/parity_minp/exec.0.pwflow << EOF
<parset name="parity_minp">
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
    <parset name="com.lgc.prowess.tool.filter.FilterTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.filter.FilterTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FLT_TYPE" type="int"> 1 </par>
        <par name="FLT_SPEC" type="int"> 1 </par>
        <par name="PHASE" type="int"> 2 </par>
        <par name="RNOISE" type="float"> 1.0 </par>
        <par name="IDOMAIN" type="int"> 2 </par>
        <par name="PCT_PAD" type="float"> 25.0 </par>
        <par name="NOTCH_OPTION" type="boolean"> false </par>
        <par name="FLT_FREQ1" type="string"> 10-20-60-80 </par>
        <par name="REMUTE_OPTION" type="boolean"> false </par>
      </parset></parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_MINP </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
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

/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_agc/exec.0.pwflow | tail -n 5
/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_butt/exec.0.pwflow | tail -n 5
/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_minp/exec.0.pwflow | tail -n 5
ls -lh "$OUT_AGC" "$OUT_BUTT" "$OUT_MINP"
