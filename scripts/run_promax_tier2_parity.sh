#!/bin/bash
set -e
export PROMAX_HOME=/apps/ProMAX
export PROWESS_HOME=/apps/SeisSpace
IN_SGY="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/input_nias_shot1000.sgy"
OUT_FX="/media/sf_Metode_Seismik_2025/rustseis/out/promax_parity/promax_fxdecon.sgy"

mkdir -p /tmp/parity_fxdecon
cat > /tmp/parity_fxdecon/exec.0.pwflow << EOF
<parset name="parity_fxdecon">
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
    <parset name="FXDECON #1">
      <par name="ToolName" type="string"> FXDECON </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters">
        <parset name="PROCDESC">
          <par name="label" type="string"> "F-X Decon" </par>
          <par name="parallel_enabled" type="boolean"> false </par>
          <par name="parallelizable" type="boolean"> true </par>
          <par name="product" type="string"> PROMAX </par>
          <par name="collapse" type="boolean"> false </par>
          <par name="ipmEnabled" type="boolean"> true </par>
          <par name="name" type="string"> fxdecon </par>
          <par name="toolType" type="string"> NORMAL </par>
          <par name="type" type="string"> Lisp </par>
          <par name="legalHybrid" type="boolean"> true </par>
        </parset>
        <parset name="execParameters">
          <parset name="FXDECON #0">
            <parset name="GENERAL">
              <par name="version" type="string"> "ProMAX 5000" </par>
              <par name="FLT_TYPE" type="long"> 1 </par>
              <par name="PCTN" type="double"> 1.0 </par>
              <par name="RADAPT" type="double"> 0.1 </par>
              <par name="NTRW" type="long"> 24 </par>
              <par name="NPF" type="long"> 5 </par>
              <par name="TIMEW" type="double"> 1000.0 </par>
              <par name="TOVLP" type="double"> 100.0 </par>
              <par name="FR1" type="double"> 6.0 </par>
              <par name="FR2" type="double"> 90.0 </par>
              <par name="NPASSES" type="long"> 1 </par>
              <par name="HDRLIMIT" type="long"> 0 </par>
              <par name="HDRNAME" type="string"> NONE </par>
              <par name="REMUTE" type="long"> 0 </par>
            </parset>
          </parset>
        </parset>
      </parset>
    </parset>
    <parset name="com.lgc.prowess.tool.segyoutput.SegYOutputTool #1">
      <par name="ToolName" type="string"> com.lgc.prowess.tool.segyoutput.SegYOutputTool </par>
      <par name="enabled" type="boolean"> true </par>
      <parset name="parameters"><parset name="menuParameters">
        <par name="FILEPATH" type="string"> $OUT_FX </par>
        <par name="FORMAT_CODE" type="int"> 5 </par>
        <par name="BYTESWAP" type="boolean"> false </par>
      </parset></parset>
    </parset>
  </parset>
  <parset name="connections">
    <parset name="connection #1">
      <par name="src" type="string"> "com.lgc.prowess.tool.segyinput.SegYInputTool #1" </par>
      <par name="dst" type="string"> "FXDECON #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
    <parset name="connection #2">
      <par name="src" type="string"> "FXDECON #1" </par>
      <par name="dst" type="string"> "com.lgc.prowess.tool.segyoutput.SegYOutputTool #1" </par>
      <par name="srcPort" type="string"> default </par>
      <par name="dstPort" type="string"> default </par>
    </parset>
  </parset>
</parset>
EOF

/apps/SeisSpace/port/bin/pw.sh java com.lgc.prowess.exec.ExecLite /tmp/parity_fxdecon/exec.0.pwflow | tail -n 12 || true
ls -lh "$OUT_FX" 2>&1 || true
