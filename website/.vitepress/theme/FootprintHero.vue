<template>
  <div class="footprint-shell" aria-label="Measured runtime memory and CPU comparison">
    <div class="footprint-bar">
      <span class="status-light" />
      <b>RUNTIME RESOURCE USE</b>
      <small>LOWER IS BETTER</small>
    </div>

    <div class="footprint-metric">
      <div class="footprint-metric-heading">
        <span>PEAK TREE MEMORY</span>
        <strong>19.11 <small>MiB</small></strong>
      </div>
      <div class="footprint-row pablo"><span>Pablo</span><i><b /></i><em>19</em></div>
      <div class="footprint-row"><span>Pi</span><i><b style="width: 56%" /></i><em>160</em></div>
      <div class="footprint-row"><span>Codex</span><i><b style="width: 69%" /></i><em>196</em></div>
      <div class="footprint-row"><span>CC</span><i><b style="width: 100%" /></i><em>286</em></div>
    </div>

    <div class="footprint-metric">
      <div class="footprint-metric-heading">
        <span>MEDIAN CPU UTILIZATION</span>
        <strong>0.37<small>%</small></strong>
      </div>
      <div class="footprint-row pablo"><span>Pablo</span><i><b /></i><em>0.37%</em></div>
      <div class="footprint-row"><span>Pi</span><i><b style="width: 53%" /></i><em>3.19%</em></div>
      <div class="footprint-row"><span>CC</span><i><b style="width: 93%" /></i><em>5.59%</em></div>
      <div class="footprint-row"><span>Codex</span><i><b style="width: 100%" /></i><em>6.04%</em></div>
    </div>

    <div class="footprint-footer">
      <span>6 TASKS · MACOS ARM64</span>
      <a href="/benchmarks">METHOD + LIMITS →</a>
    </div>
  </div>
</template>
