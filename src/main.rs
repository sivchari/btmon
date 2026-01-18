//! btmon - Bluetooth battery monitor for macOS
//!
//! This tool monitors battery levels of connected Bluetooth devices
//! using both IOBluetooth (Classic) and CoreBluetooth (BLE GATT) APIs.

use clap::Parser;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSString};
use objc2_io_bluetooth::IOBluetoothDevice;
use serde::Serialize;
use std::collections::HashMap;
use tracing::{Level, debug, info, warn};

mod gatt;

/// CLI arguments for btmon
#[derive(Parser, Debug)]
#[command(name = "btmon")]
#[command(about = "Monitor Bluetooth device battery levels on macOS")]
#[command(version)]
struct Args {
    /// Filter by device name (partial match, case-insensitive)
    #[arg(short, long)]
    device: Option<String>,

    /// Output in JSON format
    #[arg(short, long)]
    json: bool,

    /// Enable debug output
    #[arg(long)]
    debug: bool,
}

/// Battery level percentage (0-100)
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(transparent)]
pub struct BatteryLevel(u8);

impl BatteryLevel {
    /// Create a new BatteryLevel from a raw value.
    /// Returns None if value is 0 or > 100 (invalid/unavailable).
    pub fn new(value: u8) -> Option<Self> {
        if value > 0 && value <= 100 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Get the battery level as a percentage
    pub fn as_percentage(self) -> u8 {
        self.0
    }
}

impl std::fmt::Display for BatteryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}%", self.0)
    }
}

/// Bluetooth device address
#[derive(Debug, Clone)]
pub enum DeviceAddress {
    /// Classic Bluetooth MAC address
    Classic(String),
    /// BLE device (address not exposed for privacy)
    Ble,
}

impl std::fmt::Display for DeviceAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Classic(addr) => write!(f, "{addr}"),
            Self::Ble => write!(f, "BLE"),
        }
    }
}

impl Serialize for DeviceAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Classic(addr) => serializer.serialize_str(addr),
            Self::Ble => serializer.serialize_str("BLE"),
        }
    }
}

/// Represents a Bluetooth device with battery information
#[derive(Debug, Serialize)]
pub struct Device {
    /// Human-readable device name
    name: String,
    /// Bluetooth address
    address: DeviceAddress,
    /// Single battery level for standard devices
    #[serde(skip_serializing_if = "Option::is_none")]
    battery_level: Option<BatteryLevel>,
    /// Left earbud battery (AirPods, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    battery_left: Option<BatteryLevel>,
    /// Right earbud battery (AirPods, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    battery_right: Option<BatteryLevel>,
    /// Charging case battery (AirPods, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    battery_case: Option<BatteryLevel>,
}

impl Device {
    /// Check if device has any battery information
    fn has_battery_info(&self) -> bool {
        self.battery_level.is_some()
            || self.battery_left.is_some()
            || self.battery_right.is_some()
            || self.battery_case.is_some()
    }
}

/// Get battery levels from GATT Battery Service devices
fn get_gatt_devices(name_filter: Option<&str>) -> Vec<Device> {
    let gatt_devices = gatt::get_gatt_battery_devices();

    gatt_devices
        .into_iter()
        .filter_map(|(name, battery)| {
            // Apply name filter
            if let Some(filter) = name_filter
                && !name.to_lowercase().contains(filter)
            {
                return None;
            }

            let battery_level = BatteryLevel::new(battery);
            if battery_level.is_none() {
                debug!(name = %name, raw_value = battery, "Invalid battery level from GATT");
                return None;
            }

            info!(name = %name, battery = battery, "Found GATT device");

            Some(Device {
                name,
                address: DeviceAddress::Ble,
                battery_level,
                battery_left: None,
                battery_right: None,
                battery_case: None,
            })
        })
        .collect()
}

/// Get battery levels from IOBluetooth devices (Classic Bluetooth)
fn get_iobluetooth_devices(
    name_filter: Option<&str>,
    seen_names: &HashMap<String, ()>,
) -> Vec<Device> {
    let mut devices = Vec::new();

    // SAFETY: IOBluetoothDevice::pairedDevices() returns a valid NSArray or nil.
    // This is a standard Objective-C API call.
    let paired_devices: Option<objc2::rc::Retained<NSArray<AnyObject>>> =
        unsafe { IOBluetoothDevice::pairedDevices() };

    let Some(paired) = paired_devices else {
        debug!("No paired devices found");
        return devices;
    };

    let count = paired.count();
    debug!(count = count, "Found paired devices");

    for i in 0..count {
        // SAFETY: objectAtIndex returns a valid pointer for valid index (0..count).
        let device: *const AnyObject = unsafe { msg_send![&paired, objectAtIndex: i] };
        if device.is_null() {
            continue;
        }

        // SAFETY: device pointer was checked for null above.
        // The object is retained by the NSArray for the duration of iteration.
        let device_ref = unsafe { &*device };

        // SAFETY: isConnected is a standard IOBluetoothDevice method returning bool.
        let is_connected: bool = unsafe { msg_send![device_ref, isConnected] };
        if !is_connected {
            continue;
        }

        // SAFETY: name returns NSString or nil.
        let name_obj: *const NSString = unsafe { msg_send![device_ref, name] };
        let name = if name_obj.is_null() {
            continue;
        } else {
            // SAFETY: name_obj was checked for null above.
            unsafe { (*name_obj).to_string() }
        };

        // Skip if already got battery from GATT
        if seen_names.contains_key(&name) {
            debug!(name = %name, "Skipping device already found via GATT");
            continue;
        }

        // Apply name filter
        if let Some(filter) = name_filter
            && !name.to_lowercase().contains(filter)
        {
            continue;
        }

        // SAFETY: addressString returns NSString or nil.
        let addr_obj: *const NSString = unsafe { msg_send![device_ref, addressString] };
        let address = if addr_obj.is_null() {
            DeviceAddress::Classic("unknown".to_string())
        } else {
            // SAFETY: addr_obj was checked for null above.
            DeviceAddress::Classic(unsafe { (*addr_obj).to_string() })
        };

        // SAFETY: These are private IOBluetooth APIs that return u8.
        // They return 0 or 255 when battery info is unavailable.
        let battery_single: u8 = unsafe { msg_send![device_ref, batteryPercentSingle] };
        let battery_left: u8 = unsafe { msg_send![device_ref, batteryPercentLeft] };
        let battery_right: u8 = unsafe { msg_send![device_ref, batteryPercentRight] };
        let battery_case: u8 = unsafe { msg_send![device_ref, batteryPercentCase] };

        // Log additional debug info
        // SAFETY: These are private IOBluetooth APIs that return u8.
        let battery_combined: u8 = unsafe { msg_send![device_ref, batteryPercentCombined] };
        let headset_battery: u8 = unsafe { msg_send![device_ref, headsetBattery] };

        debug!(
            name = %name,
            single = battery_single,
            left = battery_left,
            right = battery_right,
            case = battery_case,
            combined = battery_combined,
            headset = headset_battery,
            "IOBluetooth battery values"
        );

        let battery_level = BatteryLevel::new(battery_single);
        let battery_left = BatteryLevel::new(battery_left);
        let battery_right = BatteryLevel::new(battery_right);
        let battery_case = BatteryLevel::new(battery_case);

        let device = Device {
            name: name.clone(),
            address,
            battery_level,
            battery_left,
            battery_right,
            battery_case,
        };

        // Skip devices with no battery info
        if !device.has_battery_info() {
            debug!(name = %name, "No battery info available");
            continue;
        }

        info!(
            name = %name,
            battery_level = ?battery_level.map(|b| b.as_percentage()),
            battery_left = ?battery_left.map(|b| b.as_percentage()),
            battery_right = ?battery_right.map(|b| b.as_percentage()),
            battery_case = ?battery_case.map(|b| b.as_percentage()),
            "Found IOBluetooth device"
        );

        devices.push(device);
    }

    devices
}

/// Get all connected Bluetooth devices with battery information
fn get_connected_devices(name_filter: Option<&str>) -> Vec<Device> {
    // Pre-convert filter to lowercase for efficiency
    let filter_lower = name_filter.map(|f| f.to_lowercase());
    let filter_ref = filter_lower.as_deref();

    // First, get GATT Battery Service devices via Core Bluetooth
    let gatt_devices = get_gatt_devices(filter_ref);

    // Track seen device names to avoid duplicates
    let seen_names: HashMap<String, ()> =
        gatt_devices.iter().map(|d| (d.name.clone(), ())).collect();

    // Then get IOBluetooth devices
    let iobluetooth_devices = get_iobluetooth_devices(filter_ref, &seen_names);

    // Merge results
    let mut devices = gatt_devices;
    devices.extend(iobluetooth_devices);

    devices
}

/// Format device output for terminal display
fn format_device_output(device: &Device) -> String {
    if let Some(level) = device.battery_level {
        format!("{}: {level}", device.name)
    } else {
        // AirPods-style device with multiple batteries
        let mut parts = Vec::new();
        if let Some(l) = device.battery_left {
            parts.push(format!("L:{l}"));
        }
        if let Some(r) = device.battery_right {
            parts.push(format!("R:{r}"));
        }
        if let Some(c) = device.battery_case {
            parts.push(format!("Case:{c}"));
        }
        format!("{}: {}", device.name, parts.join(" "))
    }
}

fn main() {
    let args = Args::parse();

    // Initialize tracing subscriber with JSON format
    if args.debug {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .json()
            .init();
    }

    debug!("Starting btmon");

    let devices = get_connected_devices(args.device.as_deref());

    if devices.is_empty() {
        if let Some(ref filter) = args.device {
            warn!(filter = %filter, "No devices found matching filter");
            eprintln!("no devices found matching '{filter}'");
        } else {
            warn!("No devices with battery info found");
            eprintln!("no devices with battery info found");
        }
        return;
    }

    if args.json {
        match serde_json::to_string_pretty(&devices) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                warn!(error = %e, "Failed to serialize devices to JSON");
                eprintln!("Failed to serialize devices: {e}");
            }
        }
    } else {
        for device in &devices {
            println!("{}", format_device_output(device));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_battery_level_valid() {
        assert!(BatteryLevel::new(1).is_some());
        assert!(BatteryLevel::new(50).is_some());
        assert!(BatteryLevel::new(100).is_some());
    }

    #[test]
    fn test_battery_level_invalid() {
        assert!(BatteryLevel::new(0).is_none());
        assert!(BatteryLevel::new(101).is_none());
        assert!(BatteryLevel::new(255).is_none());
    }

    #[test]
    fn test_battery_level_display() {
        let level = BatteryLevel::new(75).unwrap();
        assert_eq!(format!("{level}"), "75%");
    }

    #[test]
    fn test_device_has_battery_info() {
        let device_with_single = Device {
            name: "Test".to_string(),
            address: DeviceAddress::Ble,
            battery_level: BatteryLevel::new(50),
            battery_left: None,
            battery_right: None,
            battery_case: None,
        };
        assert!(device_with_single.has_battery_info());

        let device_with_left_right = Device {
            name: "AirPods".to_string(),
            address: DeviceAddress::Classic("aa:bb:cc:dd:ee:ff".to_string()),
            battery_level: None,
            battery_left: BatteryLevel::new(80),
            battery_right: BatteryLevel::new(90),
            battery_case: None,
        };
        assert!(device_with_left_right.has_battery_info());

        let device_without_battery = Device {
            name: "Mouse".to_string(),
            address: DeviceAddress::Ble,
            battery_level: None,
            battery_left: None,
            battery_right: None,
            battery_case: None,
        };
        assert!(!device_without_battery.has_battery_info());
    }

    #[test]
    fn test_format_device_output_single() {
        let device = Device {
            name: "Keyboard".to_string(),
            address: DeviceAddress::Ble,
            battery_level: BatteryLevel::new(76),
            battery_left: None,
            battery_right: None,
            battery_case: None,
        };
        assert_eq!(format_device_output(&device), "Keyboard: 76%");
    }

    #[test]
    fn test_format_device_output_airpods() {
        let device = Device {
            name: "AirPods Pro".to_string(),
            address: DeviceAddress::Classic("aa:bb:cc:dd:ee:ff".to_string()),
            battery_level: None,
            battery_left: BatteryLevel::new(80),
            battery_right: BatteryLevel::new(90),
            battery_case: BatteryLevel::new(100),
        };
        assert_eq!(
            format_device_output(&device),
            "AirPods Pro: L:80% R:90% Case:100%"
        );
    }
}
