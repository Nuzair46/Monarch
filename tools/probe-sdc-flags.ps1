# Probes which SetDisplayConfig flag combinations Windows accepts.
#
# Every call uses SDC_VALIDATE, so the probe does not apply a display configuration.
# Validation evaluates both the parameters and the requested configuration. Record the
# Windows version, GPU/driver and connected displays when comparing results.
# 87 is ERROR_INVALID_PARAMETER; 31 is ERROR_GEN_FAILURE. Neither code alone proves
# where a driver rejected the request or establishes behavior on other machines.

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class SdcProbe {
    [DllImport("user32.dll")]
    public static extern int SetDisplayConfig(
        uint numPathArrayElements, IntPtr pathArray,
        uint numModeInfoArrayElements, IntPtr modeInfoArray,
        uint flags);
}
"@

$VALIDATE     = 0x40    # SDC_VALIDATE           - applies nothing
$EXTEND       = 0x04    # SDC_TOPOLOGY_EXTEND
$CLONE        = 0x02    # SDC_TOPOLOGY_CLONE
$ALLOW        = 0x400   # SDC_ALLOW_CHANGES
$SAVE_DB      = 0x200   # SDC_SAVE_TO_DATABASE
$PERSIST      = 0x800   # SDC_PATH_PERSIST_IF_REQUIRED

function Probe($label, $flags) {
    $status = [SdcProbe]::SetDisplayConfig(0, [IntPtr]::Zero, 0, [IntPtr]::Zero, $flags)
    $meaning = switch ($status) {
        0     { "OK          - flags legal" }
        87    { "ERROR_INVALID_PARAMETER" }
        31    { "ERROR_GEN_FAILURE" }
        1168  { "ERROR_NOT_FOUND" }
        default { "status $status" }
    }
    "{0,-56} mask={1,-5} -> {2,-4} {3}" -f $label, $flags, $status, $meaning
}

Write-Output ""
Write-Output "Current code in force_topology_extend:"
Probe "VALIDATE|EXTEND|ALLOW_CHANGES|SAVE_TO_DATABASE" ($VALIDATE -bor $EXTEND -bor $ALLOW -bor $SAVE_DB)

Write-Output ""
Write-Output "Removing one flag at a time:"
Probe "VALIDATE|EXTEND|ALLOW_CHANGES|PATH_PERSIST"     ($VALIDATE -bor $EXTEND -bor $ALLOW -bor $PERSIST)
Probe "VALIDATE|EXTEND|ALLOW_CHANGES"                  ($VALIDATE -bor $EXTEND -bor $ALLOW)
Probe "VALIDATE|EXTEND|PATH_PERSIST   (proposed fix)"  ($VALIDATE -bor $EXTEND -bor $PERSIST)
Probe "VALIDATE|EXTEND"                                ($VALIDATE -bor $EXTEND)

Write-Output ""
Write-Output "Compare ALLOW_CHANGES with EXTEND and CLONE on this system:"
Write-Output "The documentation permits it with otherwise valid combinations."
Probe "VALIDATE|CLONE|ALLOW_CHANGES"                   ($VALIDATE -bor $CLONE -bor $ALLOW)
Probe "VALIDATE|CLONE"                                 ($VALIDATE -bor $CLONE)
Probe "VALIDATE|CLONE|PATH_PERSIST"                    ($VALIDATE -bor $CLONE -bor $PERSIST)
Write-Output ""
