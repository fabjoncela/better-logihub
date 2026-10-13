use std::rc::Rc;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use super::transport::{
    HidTransport, HidppError, dpi_from_be, dpi_to_be, fn_sw, long_frame, short_frame,
};

const FEATURE_ROOT: u16 = 0x0000;
const FEATURE_SET: u16 = 0x0001;
const FEATURE_FIRMWARE_INFO: u16 = 0x0003;
const FEATURE_DEVICE_NAME: u16 = 0x0005;
const FEATURE_BATTERY_LEVEL_STATUS: u16 = 0x1000;
const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
const FEATURE_ADJUSTABLE_DPI: u16 = 0x2201;
const FEATURE_EXTENDED_ADJUSTABLE_DPI: u16 = 0x2202;
const FEATURE_REPORT_RATE: u16 = 0x8060;
const FEATURE_EXTENDED_REPORT_RATE: u16 = 0x8061;
const FEATURE_ONBOARD_PROFILES: u16 = 0x8100;
const SLEEP_RETRIES: usize = 3;
const SLEEP_RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize)]
pub struct BatteryStatus {
    pub percent: u8,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureInfo {
    pub index: u8,
    pub id: u16,
    pub name: &'static str,
}

pub struct Device {
    transport: Rc<HidTransport>,
    dev_idx: u8,
}

impl Device {
    pub fn new(transport: Rc<HidTransport>, dev_idx: u8) -> Self {
        Self { transport, dev_idx }
    }

    pub fn probe(&self) -> Result<(), HidppError> {
        self.require_feature(FEATURE_SET).map(|_| ())
    }

    pub fn name(&self) -> Result<String, HidppError> {
        let feature = self.require_feature(FEATURE_DEVICE_NAME)?;
        let length = self.call(feature, 0, &[])?[0] as usize;
        let mut name = Vec::with_capacity(length);
        while name.len() < length {
            let offset = u8::try_from(name.len()).map_err(|_| {
                HidppError::Malformed("device name is longer than 255 bytes".into())
            })?;
            let response = self.call(feature, 1, &[offset])?;
            let needed = length - name.len();
            let chunk_length = needed.min(response.len());
            let chunk = &response[..chunk_length];
            let before = name.len();
            name.extend(chunk.iter().copied().take_while(|byte| *byte != 0));
            if name.len() == before {
                return Err(HidppError::Malformed(
                    "device returned an empty name chunk".into(),
                ));
            }
        }
        name.truncate(length);
        Ok(String::from_utf8_lossy(&name).into_owned())
    }

    pub fn battery(&self) -> Result<BatteryStatus, HidppError> {
        if let Some(feature) = self.get_feature(FEATURE_UNIFIED_BATTERY)? {
            let response = self.call(feature, 1, &[])?;
            return Ok(BatteryStatus {
                percent: response[0],
                status: unified_battery_status_name(response[2]).to_owned(),
            });
        }
        let feature = self.require_feature(FEATURE_BATTERY_LEVEL_STATUS)?;
        let response = self.call(feature, 0, &[])?;
        Ok(BatteryStatus {
            percent: response[0],
            status: battery_level_status_name(response[2]).to_owned(),
        })
    }

    pub fn dpi(&self) -> Result<u16, HidppError> {
        if let Some(feature) = self.get_feature(FEATURE_EXTENDED_ADJUSTABLE_DPI)? {
            let response = self.call(feature, 5, &[0])?;
            return Ok(dpi_from_be(response[1], response[2]));
        }
        let feature = self.require_feature(FEATURE_ADJUSTABLE_DPI)?;
        let response = self.call(feature, 2, &[0])?;
        Ok(dpi_from_be(response[1], response[2]))
    }

    /// Read-only DPI protocol diagnostics. Does not send any DPI setter command.
    pub fn dpi_diagnostics(&self, target_dpi: u16, verbose: bool) -> Vec<String> {
        let mut lines = vec![format!(
            "Requested DPI: {target_dpi} (diagnostics only; no write)"
        )];
        if verbose {
            self.diagnose_firmware(&mut lines);
            self.diagnose_onboard_mode(&mut lines);
        }
        for (id, label, getter) in [
            (
                FEATURE_EXTENDED_ADJUSTABLE_DPI,
                "ExtendedAdjustableDPI",
                5_u8,
            ),
            (FEATURE_ADJUSTABLE_DPI, "AdjustableDPI", 2_u8),
        ] {
            match self.get_feature(id) {
                Ok(Some(index)) => {
                    lines.push(format!(
                        "Feature {label} (0x{id:04X}): available at index 0x{index:02X}"
                    ));
                    if verbose && id == FEATURE_EXTENDED_ADJUSTABLE_DPI {
                        self.diagnose_extended_sensor(index, &mut lines);
                    }
                    match self.diagnostic_call(index, getter, &[0], verbose, 10, &mut lines) {
                        Ok(bytes) => {
                            let raw = bytes[..10]
                                .iter()
                                .map(|v| format!("{v:02X}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            lines.push(format!("  fn {getter} sensor=0 response[0..10]: {raw}"));
                            lines.push(format!(
                                "  X DPI from response[1..3]: {}",
                                dpi_from_be(bytes[1], bytes[2])
                            ));
                            if id == FEATURE_EXTENDED_ADJUSTABLE_DPI {
                                let args = extended_dpi_write_params(target_dpi, &bytes);
                                let payload = args
                                    .iter()
                                    .map(|v| format!("{v:02X}"))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                lines.push(format!(
                                    "  prospective fn 6 payload (NOT SENT): {payload}"
                                ));
                                lines.push(format!(
                                    "  candidate existing Y bytes: {:02X} {:02X}; LOD byte: {:02X}",
                                    bytes[5], bytes[6], bytes[9]
                                ));
                            }
                        }
                        Err(err) => lines.push(format!("  fn {getter} read failed: {err}")),
                    }
                }
                Ok(None) => lines.push(format!("Feature {label} (0x{id:04X}): not advertised")),
                Err(err) => lines.push(format!("Feature {label} lookup failed: {err}")),
            }
        }
        lines
    }

    fn diagnostic_call(
        &self,
        feature: u8,
        function: u8,
        params: &[u8],
        verbose: bool,
        response_len: usize,
        lines: &mut Vec<String>,
    ) -> Result<[u8; 16], HidppError> {
        if verbose {
            let request = long_frame(self.dev_idx, feature, fn_sw(function, 0), params);
            lines.push(format!(
                "  request template (SWID assigned by transport): {}",
                hex_bytes(&request[..4 + params.len()])
            ));
        }
        let result = self.call(feature, function, params);
        if verbose {
            match &result {
                Ok(response) => lines.push(format!(
                    "  response parameters[0..{response_len}]: {}",
                    hex_bytes(&response[..response_len])
                )),
                Err(error) => lines.push(format!("  response error: {error}")),
            }
        }
        result
    }

    fn diagnose_firmware(&self, lines: &mut Vec<String>) {
        let Ok(Some(feature)) = self.get_feature(FEATURE_FIRMWARE_INFO) else {
            lines.push("Firmware info (0x0003): unavailable".into());
            return;
        };
        lines.push(format!("Firmware info (0x0003): index 0x{feature:02X}"));
        // Function 0 contains a per-unit identifier, so do not log its raw response.
        match self.call(feature, 0, &[]) {
            Ok(info) if info[0] > 0 => {
                lines.push(format!("  firmware entities: {}", info[0]));
                for entity in 0..info[0].min(8) {
                    if let Ok(fw) = self.diagnostic_call(feature, 1, &[entity], true, 9, lines) {
                        let prefix = &fw[1..4];
                        if fw[0] == 0 && prefix.iter().all(u8::is_ascii_alphanumeric) {
                            let prefix = String::from_utf8_lossy(prefix);
                            lines.push(format!(
                                "  main firmware: {prefix}{:02X}.{:02X} build {:04X}",
                                fw[4],
                                fw[5],
                                u16::from_be_bytes([fw[6], fw[7]])
                            ));
                        }
                    }
                }
            }
            Ok(_) => lines.push("  no firmware entities reported".into()),
            Err(error) => lines.push(format!("  firmware read failed: {error}")),
        }
    }

    fn diagnose_onboard_mode(&self, lines: &mut Vec<String>) {
        match self.get_feature(FEATURE_ONBOARD_PROFILES) {
            Ok(Some(feature)) => {
                lines.push(format!("OnboardProfiles (0x8100): index 0x{feature:02X}"));
                match self.diagnostic_call(feature, 2, &[], true, 1, lines) {
                    Ok(response) => {
                        let mode = match response[0] {
                            1 => "onboard",
                            2 => "host",
                            _ => "unknown",
                        };
                        lines.push(format!("  current mode: {mode} (0x{:02X})", response[0]));
                    }
                    Err(error) => lines.push(format!("  mode read failed: {error}")),
                }
            }
            Ok(None) => lines.push("OnboardProfiles (0x8100): not advertised".into()),
            Err(error) => lines.push(format!("OnboardProfiles lookup failed: {error}")),
        }
    }

    fn diagnose_extended_sensor(&self, feature: u8, lines: &mut Vec<String>) {
        if let Ok(count) = self.diagnostic_call(feature, 0, &[], true, 1, lines) {
            lines.push(format!("  sensor count: {}", count[0]));
            if count[0] == 0 {
                return;
            }
        }
        if let Ok(axes) = self.diagnostic_call(feature, 1, &[0], true, 3, lines) {
            if axes[0] == 0 {
                lines.push(format!(
                    "  sensor 0 axes: X, Y={}, LOD={} (flags 0x{:02X})",
                    axes[2] & 1 != 0,
                    axes[2] & 2 != 0,
                    axes[2]
                ));
                for axis in 0..=u8::from(axes[2] & 1 != 0) {
                    let mut encoded = Vec::new();
                    for page_index in 0..8_u8 {
                        match self.diagnostic_call(
                            feature,
                            2,
                            &[0, axis, page_index],
                            true,
                            16,
                            lines,
                        ) {
                            Ok(page)
                                if page[0] == 0 && page[1] == axis && page[2] == page_index =>
                            {
                                encoded.extend_from_slice(&page[3..]);
                                if let Some(ranges) = parse_extended_dpi_ranges(&encoded) {
                                    let ranges = ranges
                                        .iter()
                                        .map(|(start, end, step)| {
                                            if start == end {
                                                start.to_string()
                                            } else {
                                                format!("{start}-{end} step {step}")
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    lines.push(format!(
                                        "  {} DPI ranges: {ranges}",
                                        if axis == 0 { "X" } else { "Y" }
                                    ));
                                    break;
                                }
                            }
                            Ok(_) => {
                                lines.push(format!(
                                    "  axis {axis} list reply did not match request"
                                ));
                                break;
                            }
                            Err(error) => {
                                lines.push(format!("  axis {axis} list read failed: {error}"));
                                break;
                            }
                        }
                    }
                }
            } else {
                lines.push("  sensor axes reply did not match sensor 0".into());
            }
        }
    }

    pub fn set_dpi(&self, dpi: u16) -> Result<(), HidppError> {
        let [high, low] = dpi_to_be(dpi);
        if let Some(feature) = self.get_feature(FEATURE_EXTENDED_ADJUSTABLE_DPI)? {
            // 0x2202 fn 6 requires sensor, X DPI, Y DPI, and LOD.
            // Preserve lift-off distance; set supported X/Y axes to the same requested DPI.
            // Only emit Y=0 for sensors that report Y=0 (unsupported independent axis).
            let current = self.call(feature, 5, &[0])?;
            let args = extended_dpi_write_params(dpi, &current);
            self.call(feature, 6, &args)?;
            return Ok(());
        }
        let feature = self.require_feature(FEATURE_ADJUSTABLE_DPI)?;
        self.call(feature, 3, &[0, high, low])?;
        Ok(())
    }

    pub fn report_rate(&self) -> Result<u32, HidppError> {
        if let Some(feature) = self.get_feature(FEATURE_EXTENDED_REPORT_RATE)? {
            let value = self.call(feature, 1, &[])?[0];
            return extended_rate_to_hz(value).ok_or_else(|| {
                HidppError::Malformed(format!("unknown extended report-rate value {value}"))
            });
        }
        let feature = self.require_feature(FEATURE_REPORT_RATE)?;
        let interval_ms = self.call(feature, 1, &[])?[0];
        if interval_ms == 0 {
            return Err(HidppError::Malformed(
                "device returned a zero report-rate interval".into(),
            ));
        }
        Ok(1000 / u32::from(interval_ms))
    }

    pub fn set_report_rate(&self, hz: u32) -> Result<(), HidppError> {
        let (feature, value) = if let Some(feature) =
            self.get_feature(FEATURE_EXTENDED_REPORT_RATE)?
        {
            let value = hz_to_extended_rate(hz).ok_or_else(|| {
                HidppError::Malformed(format!(
                    "{hz} Hz is unsupported; valid extended rates are 125, 250, 500, 1000, 2000, 4000, and 8000 Hz"
                ))
            })?;
            (feature, value)
        } else {
            let feature = self.require_feature(FEATURE_REPORT_RATE)?;
            if hz == 0 || 1000 % hz != 0 || 1000 / hz > u8::MAX as u32 {
                return Err(HidppError::Malformed(format!(
                    "{hz} Hz cannot be represented as a whole millisecond interval"
                )));
            }
            (feature, (1000 / hz) as u8)
        };

        match self.call(feature, 2, &[value]) {
            Ok(_) => Ok(()),
            Err(initial) if initial.is_protocol_error() => {
                let onboard = self.require_feature(FEATURE_ONBOARD_PROFILES)?;
                self.call(onboard, 1, &[0x02])?;
                self.call(feature, 2, &[value])?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn features(&self) -> Result<Vec<FeatureInfo>, HidppError> {
        let feature_set = self.require_feature(FEATURE_SET)?;
        let count = self.call(feature_set, 0, &[])?[0];
        let mut features = Vec::with_capacity(usize::from(count) + 1);
        features.push(FeatureInfo {
            index: 0,
            id: FEATURE_ROOT,
            name: feature_name(FEATURE_ROOT),
        });
        for index in 1..=count {
            let response = self.call(feature_set, 1, &[index])?;
            let id = u16::from_be_bytes([response[0], response[1]]);
            features.push(FeatureInfo {
                index,
                id,
                name: feature_name(id),
            });
        }
        Ok(features)
    }

    pub(crate) fn require_feature(&self, feature_id: u16) -> Result<u8, HidppError> {
        self.get_feature(feature_id)?
            .ok_or(HidppError::UnsupportedFeature(feature_id))
    }

    pub(crate) fn feature_index(&self, feature_id: u16) -> Result<Option<u8>, HidppError> {
        let params = feature_id.to_be_bytes();
        let response = self.call(0, 0, &params)?;
        Ok((response[0] != 0).then_some(response[0]))
    }

    fn get_feature(&self, feature_id: u16) -> Result<Option<u8>, HidppError> {
        self.feature_index(feature_id)
    }

    fn call(&self, feature_idx: u8, function: u8, params: &[u8]) -> Result<[u8; 16], HidppError> {
        self.call_long(feature_idx, function, params)
    }

    pub(crate) fn call_long(
        &self,
        feature_idx: u8,
        function: u8,
        params: &[u8],
    ) -> Result<[u8; 16], HidppError> {
        let request = long_frame(self.dev_idx, feature_idx, fn_sw(function, 0), params);
        let response = self.transact_with_sleep_retry(&request)?;
        let mut result = [0_u8; 16];
        result.copy_from_slice(response.params());
        Ok(result)
    }

    pub(crate) fn call_short(
        &self,
        feature_idx: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Vec<u8>, HidppError> {
        if params.len() > 3 {
            return Err(HidppError::Malformed(format!(
                "short HID++ call has {} parameter bytes, maximum is 3",
                params.len()
            )));
        }
        let mut short_params = [0_u8; 4];
        short_params[0] = fn_sw(function, 0);
        short_params[1..1 + params.len()].copy_from_slice(params);
        let request = short_frame(self.dev_idx, feature_idx, short_params);
        let response = self.transact_with_sleep_retry(&request)?;
        Ok(response.params().to_vec())
    }

    pub(crate) fn transport(&self) -> Rc<HidTransport> {
        Rc::clone(&self.transport)
    }

    pub(crate) fn device_index(&self) -> u8 {
        self.dev_idx
    }

    fn transact_with_sleep_retry(
        &self,
        request: &[u8],
    ) -> Result<super::transport::Packet, HidppError> {
        for attempt in 0..SLEEP_RETRIES {
            match self.transport.transact_hidpp20(request) {
                Err(HidppError::Hidpp10 { code: 8, .. }) if attempt + 1 < SLEEP_RETRIES => {
                    thread::sleep(SLEEP_RETRY_DELAY);
                }
                result => return result,
            }
        }
        unreachable!()
    }
}

fn unified_battery_status_name(value: u8) -> &'static str {
    match value {
        0 => "discharging",
        1 => "charging",
        2 => "charging_slow",
        3 => "full",
        4 => "error",
        _ => "unknown",
    }
}

fn battery_level_status_name(value: u8) -> &'static str {
    match value {
        0 => "discharging",
        1 => "charging",
        2 => "charging (final stage)",
        3 => "full",
        4 => "charging (below optimal speed)",
        5 => "invalid battery",
        7 => "thermal error",
        8 => "charging error",
        _ => "unknown",
    }
}

fn extended_rate_to_hz(value: u8) -> Option<u32> {
    match value {
        0 => Some(125),
        1 => Some(250),
        2 => Some(500),
        3 => Some(1000),
        4 => Some(2000),
        5 => Some(4000),
        6 => Some(8000),
        _ => None,
    }
}

fn hz_to_extended_rate(hz: u32) -> Option<u8> {
    match hz {
        125 => Some(0),
        250 => Some(1),
        500 => Some(2),
        1000 => Some(3),
        2000 => Some(4),
        4000 => Some(5),
        8000 => Some(6),
        _ => None,
    }
}

pub fn feature_name(id: u16) -> &'static str {
    match id {
        0x0000 => "IRoot",
        0x0001 => "IFeatureSet",
        0x0003 => "IFirmwareInfo",
        0x0005 => "DeviceName",
        0x0007 => "DeviceFriendlyName",
        0x0020 => "ConfigChange",
        0x0620 => "RgbHostMode",
        0x0621 => "RgbOnboardEffects",
        0x0635 => "RgbStreaming",
        0x1000 => "BatteryLevelStatus",
        0x1001 => "BatteryVoltage",
        0x1004 => "UnifiedBattery",
        0x1802 => "DeviceReset",
        0x1814 => "ChangeHost",
        0x1815 => "HostsInfo",
        0x1830 => "PowerModes",
        0x18A1 => "LedState",
        0x1982 => "Backlight",
        0x1983 => "KeyboardBacklight",
        0x1B04 => "SpecialKeys",
        0x1B05 => "FullKeyCustomization",
        0x1B10 => "ControlList",
        0x1BC0 => "ReportHidUsages",
        0x1C00 => "PersistentRemappableAction",
        0x1D4B => "WirelessDeviceStatus",
        0x1DF3 => "EquadDjDebugInfo",
        0x1E00 => "EnableHiddenFeatures",
        0x1F03 => "DeviceFriendlyName",
        0x2200 => "MousePointer",
        0x2201 => "AdjustableDPI",
        0x2202 => "ExtendedAdjustableDPI",
        0x4100 => "Encryption",
        0x4522 => "DisableKeysByUsage",
        0x4540 => "KeyboardLayout",
        0x8010 => "GKeys",
        0x8020 => "MKeys",
        0x8030 => "MacroRecordKey",
        0x8040 => "BrightnessControl",
        0x8051 => "LogiModifiers",
        0x8060 => "ReportRate",
        0x8061 => "ExtendedAdjustableReportRate",
        0x8070 => "ColorLedEffects",
        0x8071 => "RgbEffects",
        0x8080 => "PerKeyLighting",
        0x8081 => "PerKeyLightingV2",
        0x8100 => "OnboardProfiles",
        _ => "Unknown",
    }
}

// Extended DPI fn 5 result: sensor, X, default-X, Y, default-Y, LOD.
// Fn 6 writes sensor, new-X, new-Y (if supported), existing-LOD.
fn extended_dpi_write_params(dpi: u16, current: &[u8; 16]) -> [u8; 6] {
    let [high, low] = dpi.to_be_bytes();
    // Sensor advertises a concrete Y-axis DPI: keep both axes synchronized.
    // Only use Y=0 when the sensor reports that the Y axis is unsupported.
    let (y_hi, y_lo) = if current[5] == 0 && current[6] == 0 {
        (0, 0)
    } else {
        (high, low)
    };
    [0, high, low, y_hi, y_lo, current[9]]
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_extended_dpi_ranges(bytes: &[u8]) -> Option<Vec<(u16, u16, u16)>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    let mut last = 0_u16;
    while offset + 2 <= bytes.len() {
        let value = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        if value == 0 {
            return Some(ranges);
        }
        if value >> 13 == 7 {
            if offset + 4 > bytes.len() || ranges.is_empty() {
                return None;
            }
            let step = value & 0x1FFF;
            let end = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
            if step == 0 || end <= last {
                return None;
            }
            let count = (end - last) / step;
            if count == 0 {
                return None;
            }
            let range_end = last + count * step;
            ranges.push((last + step, range_end, step));
            last = range_end;
            offset += 4;
        } else {
            ranges.push((value, value, 0));
            last = value;
            offset += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_dpi_frame_keeps_linked_y_and_lod() {
        let mut current = [0_u8; 16];
        current[1] = 0x03;
        current[2] = 0x20; // current 800 DPI
        current[5] = 0;
        current[6] = 0; // Y linked to X
        current[9] = 2; // medium LOD
        assert_eq!(
            extended_dpi_write_params(1800, &current),
            [0, 0x07, 0x08, 0, 0, 2]
        );
        current[5] = 0x06;
        current[6] = 0x40; // independent 1600 Y DPI
        assert_eq!(
            extended_dpi_write_params(800, &current),
            [0, 0x03, 0x20, 0x03, 0x20, 2]
        );
    }

    #[test]
    fn parses_paged_extended_dpi_ranges() {
        let encoded = [
            0x00, 0x64, 0xE0, 0x01, 0x00, 0xC8, 0xE0, 0x02, 0x01, 0xF4, 0xE0, 0x05, 0x03, 0x20, 0,
            0,
        ];
        assert_eq!(parse_extended_dpi_ranges(&encoded[..13]), None);
        assert_eq!(
            parse_extended_dpi_ranges(&encoded),
            Some(vec![
                (100, 100, 0),
                (101, 200, 1),
                (202, 500, 2),
                (505, 800, 5),
            ])
        );
    }

    #[test]
    fn converts_extended_report_rates() {
        assert_eq!(extended_rate_to_hz(0), Some(125));
        assert_eq!(extended_rate_to_hz(6), Some(8000));
        assert_eq!(hz_to_extended_rate(1000), Some(3));
        assert_eq!(hz_to_extended_rate(144), None);
    }

    #[test]
    fn converts_unified_battery_status_separately() {
        assert_eq!(unified_battery_status_name(0), "discharging");
        assert_eq!(unified_battery_status_name(2), "charging_slow");
        assert_eq!(unified_battery_status_name(3), "full");
        assert_eq!(unified_battery_status_name(4), "error");
    }

    #[test]
    fn names_phase_a_features() {
        for (id, name) in [
            (0x0007, "DeviceFriendlyName"),
            (0x0020, "ConfigChange"),
            (0x0620, "RgbHostMode"),
            (0x0621, "RgbOnboardEffects"),
            (0x0635, "RgbStreaming"),
            (0x1802, "DeviceReset"),
            (0x1814, "ChangeHost"),
            (0x1815, "HostsInfo"),
            (0x1830, "PowerModes"),
            (0x18A1, "LedState"),
            (0x1982, "Backlight"),
            (0x1983, "KeyboardBacklight"),
            (0x1B04, "SpecialKeys"),
            (0x1B05, "FullKeyCustomization"),
            (0x1B10, "ControlList"),
            (0x1BC0, "ReportHidUsages"),
            (0x1C00, "PersistentRemappableAction"),
            (0x1D4B, "WirelessDeviceStatus"),
            (0x1E00, "EnableHiddenFeatures"),
            (0x2202, "ExtendedAdjustableDPI"),
            (0x4100, "Encryption"),
            (0x4522, "DisableKeysByUsage"),
            (0x4540, "KeyboardLayout"),
            (0x8010, "GKeys"),
            (0x8020, "MKeys"),
            (0x8030, "MacroRecordKey"),
            (0x8040, "BrightnessControl"),
            (0x8051, "LogiModifiers"),
            (0x8061, "ExtendedAdjustableReportRate"),
            (0x8070, "ColorLedEffects"),
            (0x8071, "RgbEffects"),
            (0x8080, "PerKeyLighting"),
            (0x8081, "PerKeyLightingV2"),
        ] {
            assert_eq!(feature_name(id), name);
        }
    }
}
