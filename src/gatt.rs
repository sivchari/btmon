//! GATT Battery Service reading via Core Bluetooth
//!
//! This module handles reading battery levels from BLE devices that expose
//! the standard GATT Battery Service (UUID: 0x180F).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCharacteristic, CBManagerState, CBPeripheral,
    CBPeripheralDelegate, CBService, CBUUID,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, trace, warn};

/// Battery Service UUID (0x180F)
const BATTERY_SERVICE_UUID: &str = "180F";

/// Battery Level Characteristic UUID (0x2A19)
const BATTERY_LEVEL_UUID: &str = "2A19";

/// Timeout for GATT discovery operations
const GATT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Run loop iteration interval
const RUN_LOOP_INTERVAL: f64 = 0.1;

/// Internal state for the delegate
#[derive(Default)]
struct DelegateState {
    battery_levels: HashMap<String, u8>,
    peripherals_to_read: Vec<Retained<CBPeripheral>>,
    pending_reads: usize,
    done: bool,
}

/// Ivars for the Objective-C delegate class
struct DelegateIvars {
    state: RefCell<DelegateState>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BtmonCentralDelegate"]
    #[ivars = DelegateIvars]
    struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn central_manager_did_connect_peripheral(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
        ) {
            // SAFETY: peripheral.name() is a standard Core Bluetooth API.
            let name = unsafe { peripheral.name() };
            debug!(name = ?name, "Connected to peripheral");

            // Now discover services
            // SAFETY: discoverServices is a standard Core Bluetooth API.
            // We pass an array containing only the Battery Service UUID.
            unsafe {
                peripheral.discoverServices(Some(&NSArray::from_retained_slice(&[
                    CBUUID::UUIDWithString(&NSString::from_str(BATTERY_SERVICE_UUID)),
                ])));
            }
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn central_manager_did_fail_to_connect_peripheral(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            // SAFETY: peripheral.name() is a standard Core Bluetooth API.
            let name = unsafe { peripheral.name() };
            warn!(name = ?name, error = ?error, "Failed to connect to peripheral");
            self.decrement_pending();
        }

        #[unsafe(method(centralManagerDidUpdateState:))]
        fn central_manager_did_update_state(&self, central: &CBCentralManager) {
            // SAFETY: central.state() is a standard Core Bluetooth API.
            let state = unsafe { central.state() };
            debug!(state = ?state, "Central manager state updated");

            if state == CBManagerState::PoweredOn {
                self.handle_powered_on(central);
            } else if state == CBManagerState::Unauthorized || state == CBManagerState::Unsupported
            {
                warn!(state = ?state, "Bluetooth not available");
                self.ivars().state.borrow_mut().done = true;
            }
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        unsafe fn peripheral_did_discover_services(
            &self,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            if let Some(e) = error {
                warn!(error = ?e, "Error discovering services");
                self.decrement_pending();
                return;
            }

            // SAFETY: peripheral.services() is a standard Core Bluetooth API.
            unsafe {
                if let Some(services) = peripheral.services() {
                    for i in 0..services.count() {
                        let service: &CBService = &services.objectAtIndex(i);
                        let uuid = service.UUID();
                        trace!(uuid = ?uuid, "Found service");

                        // Discover battery level characteristic
                        peripheral.discoverCharacteristics_forService(
                            Some(&NSArray::from_retained_slice(&[CBUUID::UUIDWithString(
                                &NSString::from_str(BATTERY_LEVEL_UUID),
                            )])),
                            service,
                        );
                    }
                } else {
                    self.decrement_pending();
                }
            }
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        unsafe fn peripheral_did_discover_characteristics(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(e) = error {
                warn!(error = ?e, "Error discovering characteristics");
                self.decrement_pending();
                return;
            }

            // SAFETY: service.characteristics() is a standard Core Bluetooth API.
            unsafe {
                if let Some(characteristics) = service.characteristics() {
                    for i in 0..characteristics.count() {
                        let characteristic: &CBCharacteristic = &characteristics.objectAtIndex(i);
                        trace!(uuid = ?characteristic.UUID(), "Found characteristic");

                        // Read the battery level
                        peripheral.readValueForCharacteristic(characteristic);
                    }
                } else {
                    self.decrement_pending();
                }
            }
        }

        #[unsafe(method(peripheral:didUpdateValueForCharacteristic:error:))]
        unsafe fn peripheral_did_update_value(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            if let Some(e) = error {
                warn!(error = ?e, "Error reading characteristic");
                self.decrement_pending();
                return;
            }

            // SAFETY: characteristic.value() is a standard Core Bluetooth API.
            unsafe {
                if let Some(value) = characteristic.value() {
                    let len = value.length();
                    if len > 0 {
                        // Read the first byte as battery level
                        let mut battery_level: u8 = 0;
                        // SAFETY: getBytes:length: copies bytes from NSData to our buffer.
                        // We ensure the buffer is valid and the length is correct.
                        let _: () = msg_send![&value, getBytes: &mut battery_level as *mut u8, length: 1usize];

                        let name = peripheral
                            .name()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "Unknown".to_string());

                        debug!(name = %name, battery_level = battery_level, "Read battery level");

                        self.ivars()
                            .state
                            .borrow_mut()
                            .battery_levels
                            .insert(name, battery_level);
                    }
                }
            }

            self.decrement_pending();
        }
    }
);

impl CentralDelegate {
    /// Create a new CentralDelegate instance
    fn new() -> Retained<Self> {
        let this = Self::alloc();
        let this = this.set_ivars(DelegateIvars {
            state: RefCell::new(DelegateState::default()),
        });
        // SAFETY: Calling [super init] on a properly allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }

    /// Check if all operations are complete
    fn is_done(&self) -> bool {
        self.ivars().state.borrow().done
    }

    /// Take the collected battery levels
    fn take_results(&self) -> HashMap<String, u8> {
        std::mem::take(&mut self.ivars().state.borrow_mut().battery_levels)
    }

    /// Decrement pending reads counter and mark done if zero
    fn decrement_pending(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.pending_reads > 0 {
            state.pending_reads -= 1;
        }
        if state.pending_reads == 0 {
            state.done = true;
        }
    }

    /// Handle the PoweredOn state - retrieve and connect to peripherals
    fn handle_powered_on(&self, central: &CBCentralManager) {
        // SAFETY: CBUUID::UUIDWithString is a standard Core Bluetooth API.
        let battery_uuid =
            unsafe { CBUUID::UUIDWithString(&NSString::from_str(BATTERY_SERVICE_UUID)) };
        let services: Retained<NSArray<CBUUID>> = NSArray::from_retained_slice(&[battery_uuid]);

        // SAFETY: retrieveConnectedPeripheralsWithServices is a standard Core Bluetooth API.
        let connected: Retained<NSArray<CBPeripheral>> =
            unsafe { central.retrieveConnectedPeripheralsWithServices(&services) };

        let count = connected.count();
        debug!(
            count = count,
            "Found connected peripherals with Battery Service"
        );

        if count == 0 {
            self.ivars().state.borrow_mut().done = true;
            return;
        }

        self.ivars().state.borrow_mut().pending_reads = count;

        for i in 0..count {
            // SAFETY: objectAtIndex returns a valid pointer for valid index.
            // We retain the peripheral to ensure it lives long enough.
            let peripheral: Option<Retained<CBPeripheral>> = unsafe {
                let p: *const CBPeripheral = msg_send![&connected, objectAtIndex: i];
                Retained::retain(p as *mut CBPeripheral)
            };

            let Some(peripheral) = peripheral else {
                self.decrement_pending();
                continue;
            };

            // SAFETY: peripheral.name() is a standard Core Bluetooth API.
            let name = unsafe { peripheral.name() };
            trace!(name = ?name, "Processing peripheral");

            // Set delegate and connect
            // SAFETY: setDelegate and connectPeripheral_options are standard Core Bluetooth APIs.
            unsafe {
                let delegate: *const ProtocolObject<dyn CBPeripheralDelegate> =
                    ProtocolObject::from_ref(self);
                peripheral.setDelegate(Some(&*delegate));
                central.connectPeripheral_options(&peripheral, None);
            }

            self.ivars()
                .state
                .borrow_mut()
                .peripherals_to_read
                .push(peripheral);
        }
    }
}

/// Run the NSRunLoop for a short interval
fn run_loop_once() {
    // SAFETY: These are standard Foundation/AppKit APIs for running the event loop.
    unsafe {
        let run_loop: *const AnyObject = msg_send![objc2::class!(NSRunLoop), currentRunLoop];
        let date: *const AnyObject =
            msg_send![objc2::class!(NSDate), dateWithTimeIntervalSinceNow: RUN_LOOP_INTERVAL];
        let _: () = msg_send![run_loop, runUntilDate: date];
    }
}

/// Get battery levels from GATT Battery Service devices.
///
/// This function creates a CBCentralManager, retrieves connected peripherals
/// that advertise the Battery Service, and reads their battery levels.
///
/// # Returns
///
/// A HashMap mapping device names to their battery levels (0-100).
pub fn get_gatt_battery_devices() -> HashMap<String, u8> {
    let delegate = CentralDelegate::new();

    // SAFETY: CBCentralManager initialization is a standard Core Bluetooth API.
    // We pass our delegate and a nil queue (uses main queue).
    let _central: Retained<CBCentralManager> = unsafe {
        let delegate_obj: *const ProtocolObject<dyn CBCentralManagerDelegate> =
            ProtocolObject::from_ref(&*delegate);
        msg_send![CBCentralManager::alloc(), initWithDelegate: delegate_obj, queue: std::ptr::null::<AnyObject>()]
    };

    let start = Instant::now();

    while !delegate.is_done() && start.elapsed() < GATT_DISCOVERY_TIMEOUT {
        run_loop_once();
    }

    if !delegate.is_done() {
        warn!(
            elapsed_ms = start.elapsed().as_millis(),
            "Timeout waiting for GATT battery levels"
        );
    }

    delegate.take_results()
}
