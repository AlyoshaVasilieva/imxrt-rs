//! Direct Memory Access (DMA)
//!
//! Users may wrap iMXRT peripherals in a [`Peripheral`](struct.Peripheral.html)
//! to perform DMA transfers to and from the device. A `Peripheral` accepts up
//! to two DMA [`Channel`](struct.Channel.html)s. One channel can support either
//! a peripheral-to-memory transfer, or a memory-to-peripheral transfer. When
//! constructing a `Peripheral`, you may configure the transfer completion to
//! generate an interrupt.
//!
//! # Terms
//!
//! - *Source* is a location in memory that provides data. A source may be a buffer
//!    of data, or a peripheral register.
//! - *Destination* is a location in memory that will receive data.
//!   A destination may be a buffer of data, or a peripheral register.
//! - *Transfer* is an overloaded term, meaning either a DMA transfer, or the movement
//!   of data out of software, through a peripheral, to an external device.
//! - *DMA Transfer* is an operation achieved by the DMA controller to move data from a
//!   source to a destination.
//! - *Receive* means that we're moving data into software, through a peripheral, from an
//!   external device.
//!
//! ## TODO
//!
//! - Channel arbitration modes
//! - Channel grouping
//! - Channel priority, and channel priority swapping
//! - Channel chaining
//! - Indivisible transfers (transfers that have one major loop, and multiple minor loops)

#![allow(non_snake_case)] // Compatibility with RAL

mod element;
pub(crate) mod peripheral;
mod register;

pub use element::Element;
pub use peripheral::{Destination, Source};

use crate::{ccm, ral};
use core::{
    fmt::{self, Debug, Display},
    mem,
};
use register::{DMARegisters, MultiplexerRegisters, Static, DMA, MULTIPLEXER};

/// A DMA channel
///
/// DMA channels provide one-way transfers of data. They accept a source of data,
/// and a destination of data. They copy data from the source to the destination.
/// When the transfer is complete, a DMA channel signals completion by changing a
/// value in a register, or triggering an interrupt.
///
/// DMA channels have very little public interface. They're best used when paired with a
/// [`Peripheral`](struct.Peripheral.html).
pub struct Channel {
    /// Our channel number, expected to be between 0 to 31
    index: usize,
    /// Reference to the DMA registers
    registers: Static<DMARegisters>,
    /// Reference to the DMA multiplexer
    multiplexer: Static<MultiplexerRegisters>,
}

impl Channel {
    /// Allocates a DMA channel, and sets the initial state for
    /// the channel.
    fn new(index: usize) -> Self {
        let channel = Channel {
            index,
            registers: DMA,
            multiplexer: MULTIPLEXER,
        };
        channel.registers.TCD[channel.index].reset();
        channel
    }

    /// Safety: lifetime of `source` must be greater than the lifetime
    /// of the DMA transfer.
    unsafe fn set_source<E: Element>(&mut self, source: *const E) {
        let tcd = &self.registers.TCD[self.index];
        ral::write_reg!(register::tcd, tcd, SADDR, source as u32);
        ral::write_reg!(register::tcd, tcd, SOFF, 0);
        ral::modify_reg!(register::tcd, tcd, ATTR, SSIZE: E::DATA_TRANSFER_ID, SMOD: 0);
        ral::write_reg!(register::tcd, tcd, NBYTES, mem::size_of::<E>() as u32);
        ral::write_reg!(register::tcd, tcd, SLAST, 0);
    }

    /// Safety: lifetime of 'destination' must be greater than the lifetime
    /// of the DMA transfer.
    unsafe fn set_destination<E: Element>(&mut self, destination: *const E) {
        let tcd = &self.registers.TCD[self.index];
        ral::write_reg!(register::tcd, tcd, DADDR, destination as u32);
        ral::write_reg!(register::tcd, tcd, DOFF, 0);
        ral::modify_reg!(register::tcd, tcd, ATTR, DSIZE: E::DATA_TRANSFER_ID, DMOD: 0);
        ral::write_reg!(register::tcd, tcd, NBYTES, mem::size_of::<E>() as u32);
        ral::write_reg!(register::tcd, tcd, DLAST_SGA, 0);
    }

    /// Safety: lifetime of 'source' must be greater than the lifetime of the
    /// DMA transfer.
    unsafe fn set_source_buffer<E: Element>(&mut self, source: &[E]) {
        let tcd = &self.registers.TCD[self.index];
        ral::write_reg!(register::tcd, tcd, SADDR, source.as_ptr() as u32);
        ral::write_reg!(register::tcd, tcd, SOFF, mem::size_of::<E>() as i16);
        ral::modify_reg!(register::tcd, tcd, ATTR, SSIZE: E::DATA_TRANSFER_ID, SMOD: 0);
        ral::write_reg!(register::tcd, tcd, NBYTES, mem::size_of::<E>() as u32);
        ral::write_reg!(
            register::tcd,
            tcd,
            SLAST,
            (source.len() as i32).wrapping_neg()
        );

        let iterations = source.len() as u16;
        ral::write_reg!(register::tcd, tcd, CITER, iterations);
        ral::write_reg!(register::tcd, tcd, BITER, iterations);
    }

    /// Safety: lifetime of 'destination' must be greater than the lifetime of
    /// the DMA transfer
    unsafe fn set_desination_buffer<E: Element>(&mut self, destination: &mut [E]) {
        let tcd = &self.registers.TCD[self.index];
        ral::write_reg!(register::tcd, tcd, DADDR, destination.as_mut_ptr() as u32);
        ral::write_reg!(register::tcd, tcd, DOFF, mem::size_of::<E>() as i16);
        ral::modify_reg!(register::tcd, tcd, ATTR, DSIZE: E::DATA_TRANSFER_ID, DMOD: 0);
        ral::write_reg!(register::tcd, tcd, NBYTES, mem::size_of::<E>() as u32);
        ral::write_reg!(
            register::tcd,
            tcd,
            DLAST_SGA,
            (destination.len() as i32).wrapping_neg()
        );

        let iterations = destination.len() as u16;
        ral::write_reg!(register::tcd, tcd, CITER, iterations);
        ral::write_reg!(register::tcd, tcd, BITER, iterations);
    }

    /// Enable or disabling triggering from hardware
    ///
    /// If source is `Some(value)`, we trigger from hardware identified by the source identifier.
    /// If `source` is `None`, we disable hardware triggering.
    fn set_trigger_from_hardware(&mut self, source: Option<u32>) {
        let chcfg = &self.multiplexer.chcfg[self.index];
        chcfg.write(0);
        if let Some(source) = source {
            const ENBL: u32 = 1 << 31;
            chcfg.write(ENBL | source);
        }
    }

    /// Returns `true` if the DMA channel is receiving a service signal from hardware
    fn hardware_signaling(&self) -> bool {
        self.registers.HRS.read() & (1 << self.index) != 0
    }

    /// Enable or disable the DMA channel
    fn set_enable(&mut self, enable: bool) {
        if enable {
            self.registers.SERQ.write(self.index as u8);
        } else {
            self.registers.CERQ.write(self.index as u8);
        }
    }

    /// Returns `true` if this DMA channel generated an interrupt
    fn interrupt(&self) -> bool {
        self.registers.INT.read() & (1 << self.index) != 0
    }

    /// Clear the interrupt flag from this DMA channel
    fn clear_interrupt(&mut self) {
        self.registers.CINT.write(self.index as u8);
    }

    /// Enable or disable 'disable on completion'
    ///
    /// 'Disable on completion' lets the DMA channel automatically clear the request signal
    /// when it completes a transfer.
    fn set_disable_on_completion(&mut self, dreq: bool) {
        let tcd = &self.registers.TCD[self.index];
        ral::modify_reg!(register::tcd, tcd, CSR, DREQ: dreq as u16);
    }

    /// Enable or disable interrupt generation when the transfer completes
    fn set_interrupt_on_completion(&mut self, intr: bool) {
        let tcd = &self.registers.TCD[self.index];
        ral::modify_reg!(register::tcd, tcd, CSR, INTMAJOR: intr as u16);
    }

    /// Returns `true` if this channel's completion will generate an interrupt
    fn interrupt_on_completion(&self) -> bool {
        let tcd = &self.registers.TCD[self.index];
        ral::read_reg!(register::tcd, tcd, CSR, INTMAJOR == 1)
    }

    /// Indicates if the DMA transfer has completed
    fn complete(&self) -> bool {
        let tcd = &self.registers.TCD[self.index];
        ral::read_reg!(register::tcd, tcd, CSR, DONE == 1)
    }

    /// Clears completion indication
    fn clear_complete(&mut self) {
        self.registers.CDNE.write(self.index as u8);
    }

    /// Indicates if the DMA channel is in an error state
    fn error(&self) -> bool {
        self.registers.ERR.read() & (1 << self.index) != 0
    }

    /// Clears the error flag
    fn clear_error(&mut self) {
        self.registers.CERR.write(self.index as u8);
    }

    /// Indicates if this DMA channel is active
    fn active(&self) -> bool {
        let tcd = &self.registers.TCD[self.index];
        ral::read_reg!(register::tcd, tcd, CSR, ACTIVE == 1)
    }

    /// Returns the value from the **global** error status register
    ///
    /// It may reflect the last channel that produced an error, and that
    /// may not be related to this channel.
    fn error_status(&self) -> u32 {
        self.registers.ES.read()
    }
}

/// A DMA-capable peripheral
///
/// `Peripheral` wraps an object that can act as a source and / or destination
/// for a DMA transfer. It provides an interface for scheduling transfers, and
/// for knowing when transfers are complete.
///
/// The most useful methods are [`start_transfer()`](struct.Peripheral.html#method.start_transfer)
/// and [`start_receive()`](struct.Peripheral.html#method.start_receive). Each method accepts
/// a buffer, and will either
///
/// - send data from the buffer to the peripheral (`start_transfer()`)
/// - move data from a peripheral into the buffer (`start_receive()`)
///
/// Both methods are unsafe! You're responsible for making sure the lifetime of the buffers is
/// greater than the lifetime of the transfer.
///
/// When constructing a `Peripheral`, you may supply a configuration to trigger an interrupt when
/// the DMA transfer completes. If you enable interrupts, you're responsible for registering the
/// interrupt, and for clearing the interrupt. The `Peripheral` has methods for clearing interrupts
/// due to transfer and receive DMA channels.
pub struct Peripheral<P, E> {
    /// Channel used for outgoing data (from software, to external device)
    tx_channel: Option<Channel>,
    /// Channel used for incoming data (from external device, to software)
    rx_channel: Option<Channel>,
    /// The peripheral that is either providing the data, or accepting the data,
    /// or both.
    peripheral: P,
    _element: core::marker::PhantomData<E>,
}

#[derive(Clone, Copy)]
/// Configurations for defining DMA transfers
///
/// Use [`ConfigBuilder`](struct.ConfigBuilder.html) to create a DMA
/// transfer configuration.
pub struct Config {
    interrupt_on_completion: bool,
    // Developer note: update `Config::from_channel` when
    // adding more fields!
}

impl Config {
    /// Returns a `Config` that represents the state of the supplied channel
    fn from_channel(channel: &Channel) -> Self {
        Config {
            interrupt_on_completion: channel.interrupt_on_completion(),
        }
    }
}

/// Builder for defining your DMA configuration
pub struct ConfigBuilder(Config);

impl Default for ConfigBuilder {
    fn default() -> Self {
        ConfigBuilder::new()
    }
}

impl ConfigBuilder {
    /// Construct a builder, and begin defining a configuration
    pub fn new() -> Self {
        ConfigBuilder(Config {
            interrupt_on_completion: false,
        })
    }

    /// Specifies that this DMA channel will trigger an interrupt
    /// when the transfer completes.
    ///
    /// The actual interrupt that will trigger depends on the supplied
    /// channel. There are 15 interrupts for DMA channels, and each
    /// interrupt supports two channels. You're responsible for managing
    /// the interrupts, and for registering your handler for the correct
    /// DMA channel.
    pub fn interrupt_on_completion(mut self, interrupt_on_completion: bool) -> Self {
        self.0.interrupt_on_completion = interrupt_on_completion;
        self
    }

    /// Complete configuration, and return a `Config` for a DMA transfer
    pub fn build(self) -> Config {
        self.0
    }
}

/// An error when preparing a transfer
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<P> {
    /// There is already an active transfer
    ///
    /// Cancel the transfer and try again.
    ActiveTransfer,
    /// The peripheral returned an error
    Peripheral(P),
    /// Error in setting up the DMA transfer
    Setup(ErrorStatus),
}

impl<P> From<P> for Error<P> {
    fn from(error: P) -> Self {
        Error::Peripheral(error)
    }
}

impl<P, E> Peripheral<P, E> {
    fn new(peripheral: P) -> Self {
        Peripheral {
            peripheral,
            rx_channel: None,
            tx_channel: None,
            _element: core::marker::PhantomData,
        }
    }
}

impl<P, E> Peripheral<P, E>
where
    P: Source<E>,
    E: Element,
{
    /// Wraps a peripheral that can act as the source of a DMA transfer
    pub fn new_receive(source: P, channel: Channel, config: Config) -> Self {
        let mut peripheral = Peripheral::new(source);
        peripheral.init_receive(channel, config);
        peripheral
    }

    fn init_receive(&mut self, mut channel: Channel, config: Config) {
        channel.set_trigger_from_hardware(Some(P::SOURCE_REQUEST_SIGNAL));
        // Safety: Source trait is only implemented on peripherals within
        // this crate. We may study those implementations to show that the
        // pointers point to valid memory.
        unsafe {
            channel.set_source(self.peripheral.source());
        }
        channel.set_interrupt_on_completion(config.interrupt_on_completion);
        channel.set_disable_on_completion(true);
        self.rx_channel = Some(channel);
    }

    /// Start a DMA transfer that transfers data from the peripheral into the supplied buffer
    ///
    /// A complete transfer is signaled by `receive_complete()`, and possibly an interrupt.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the lifetime of the buffer is greater than the lifetime
    /// of the transfer.
    pub unsafe fn start_receive(&mut self, buffer: &mut [E]) -> Result<(), Error<P::Error>> {
        let rx_channel = self.rx_channel.as_mut().unwrap();
        if rx_channel.active() {
            return Err(Error::ActiveTransfer);
        }
        rx_channel.set_desination_buffer(buffer);
        self.peripheral.enable_source()?;
        rx_channel.set_enable(true);
        if rx_channel.error() {
            let es = ErrorStatus::new(rx_channel.error_status());
            rx_channel.clear_error();
            Err(Error::Setup(es))
        } else {
            Ok(())
        }
    }

    /// Returns `true` if the receive is complete
    pub fn receive_complete(&self) -> bool {
        self.rx_channel.as_ref().unwrap().complete()
    }

    /// Clears the flag that indicates the DMA transfer is complete, and
    /// disable the peripheral.
    ///
    /// Users are **required** to call this to disable the source. Otherwise,
    /// the source may continue to generate DMA requests.
    pub fn receive_clear_complete(&mut self) {
        self.rx_channel.as_mut().unwrap().clear_complete();
        self.peripheral.disable_source();
    }

    /// Indicates if the receive channel has generated an interrupt
    pub fn receive_interrupt(&self) -> bool {
        self.rx_channel.as_ref().unwrap().interrupt()
    }

    /// Clears the interrupt flag on the receive channel
    ///
    /// Users are **required** to clear the interrupt flag, or the hardware
    /// may continue to generate interrupts for the channel.
    pub fn receive_clear_interrupt(&mut self) {
        self.rx_channel.as_mut().unwrap().clear_interrupt()
    }

    /// Cancel a receive transfer
    pub fn receive_cancel(&mut self) {
        self.peripheral.disable_source();
        let rx_channel = self.rx_channel.as_mut().unwrap();
        while rx_channel.hardware_signaling() {
            core::sync::atomic::spin_loop_hint();
        }
        rx_channel.set_enable(false);
    }

    /// Returns a copy of the config used to create the receive
    /// peripheral
    pub fn receive_config(&self) -> Config {
        let rx_channel = self.rx_channel.as_ref().unwrap();
        Config::from_channel(rx_channel)
    }

    /// Release the peripheral and the channel
    ///
    /// Users should ensure that any started transfer has completed. If the
    /// `Peripheral` was constructed with [`new_transfer_receive()`](struct.Peripheral.html#method.new_transfer_receive),
    /// callers should use [`release_transfer_receive()`](struct.Peripheral.html#method.release_transfer_receive);
    /// otherwise, the transfer channel will be dropped when this method returns.
    ///
    /// To get a copy of the original config, use [`receive_config()`](struct.Peripheral.html#method.receive_config)
    /// before releasing the object.
    pub fn receive_release(mut self) -> (P, Channel) {
        (self.peripheral, self.rx_channel.take().unwrap())
    }
}

/// Create a peripheral that can suppy `u8` data for DMA transfers
pub fn receive_u8<P>(source: P, channel: Channel, config: Config) -> Peripheral<P, u8>
where
    P: Source<u8>,
{
    Peripheral::new_receive(source, channel, config)
}

/// Create a peripheral that can supply `u16` data for DMA transfers
pub fn receive_u16<P>(source: P, channel: Channel, config: Config) -> Peripheral<P, u16>
where
    P: Source<u16>,
{
    Peripheral::new_receive(source, channel, config)
}

impl<P, E> Peripheral<P, E>
where
    P: Destination<E>,
    E: Element,
{
    /// Wraps a peripheral that can act as the destination of a DMA transfer
    pub fn new_transfer(destination: P, channel: Channel, config: Config) -> Self {
        let mut peripheral = Peripheral::new(destination);
        peripheral.init_transfer(channel, config);
        peripheral
    }

    fn init_transfer(&mut self, mut channel: Channel, config: Config) {
        channel.set_trigger_from_hardware(Some(P::DESTINATION_REQUEST_SIGNAL));
        // Safety: Destination trait is only implemented on peripherals within
        // this crate. We may study those implementations to show that the pointers
        // point to valid memory.
        unsafe {
            channel.set_destination(self.peripheral.destination());
        }
        channel.set_interrupt_on_completion(config.interrupt_on_completion);
        channel.set_disable_on_completion(true);
        self.tx_channel = Some(channel);
    }

    /// Start a DMA transfer that transfers data from the supplied buffer to the peripheral
    ///
    /// A complete transfer is signaled by `transfer_complete()`, and possibly an interrupt.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the lifetime of the buffer is greater than the lifetime
    /// of the transfer.
    pub unsafe fn start_transfer(&mut self, buffer: &[E]) -> Result<(), Error<P::Error>> {
        let tx_channel = self.tx_channel.as_mut().unwrap();
        if tx_channel.active() {
            return Err(Error::ActiveTransfer);
        }
        tx_channel.set_source_buffer(buffer);
        self.peripheral.enable_destination()?;
        tx_channel.set_enable(true);
        if tx_channel.error() {
            let es = ErrorStatus::new(tx_channel.error_status());
            tx_channel.clear_error();
            Err(Error::Setup(es))
        } else {
            Ok(())
        }
    }

    /// Returns `true` if the transfer is complete
    pub fn transfer_complete(&self) -> bool {
        self.tx_channel.as_ref().unwrap().complete()
    }

    /// Clears the flag that indicates the DMA transfer is complete, and
    /// disable the peripheral.
    ///
    /// Users are **required** to call this to disable the source. Otherwise,
    /// the source may continue to generate DMA requests.
    pub fn transfer_clear_complete(&mut self) {
        self.tx_channel.as_mut().unwrap().clear_complete();
        self.peripheral.disable_destination();
    }

    /// Indicates if the transfer channel has generated an interrupt
    pub fn transfer_interrupt(&self) -> bool {
        self.tx_channel.as_ref().unwrap().interrupt()
    }

    /// Clears the interrupt flag on the transfer channel
    ///
    /// Users are **required** to clear the interrupt flag, or the hardware
    /// may continue to generate interrupts for the channel.
    pub fn transfer_clear_interrupt(&mut self) {
        self.tx_channel.as_mut().unwrap().clear_interrupt()
    }

    /// Cancel a transfer that sends data to the peripheral
    pub fn transfer_cancel(&mut self) {
        self.peripheral.disable_destination();
        let tx_channel = self.tx_channel.as_mut().unwrap();
        while tx_channel.hardware_signaling() {
            core::sync::atomic::spin_loop_hint();
        }
        tx_channel.set_enable(false);
    }

    /// Returns a copy of the transfer config supplied during
    /// construction
    pub fn transfer_config(&self) -> Config {
        let tx_channel = self.tx_channel.as_ref().unwrap();
        Config::from_channel(tx_channel)
    }

    /// Release the peripheral and the channel
    ///
    /// Users should ensure that any started transfer has completed. If the
    /// `Peripheral` was constructed with [`new_transfer_receive()`](struct.Peripheral.html#method.new_transfer_receive),
    /// callers should use [`transfer_receive_release()`](struct.Peripheral.html#method.transfer_receive_release);
    /// otherwise, the receiver channel will be dropped when this method returns.
    ///
    /// To get a copy of the original config, use [`transfer_config()`](struct.Peripheral.html#method.transfer_config)
    /// before releasing the object.
    pub fn transfer_release(mut self) -> (P, Channel) {
        (self.peripheral, self.tx_channel.take().unwrap())
    }
}

/// Create a peripheral that can accept `u8` data from DMA transfers
pub fn transfer_u8<P>(destination: P, channel: Channel, config: Config) -> Peripheral<P, u8>
where
    P: Destination<u8>,
{
    Peripheral::new_transfer(destination, channel, config)
}

/// Create a peripheral that can accept `u16` data from DMA transfers
pub fn transfer_u16<P>(destination: P, channel: Channel, config: Config) -> Peripheral<P, u16>
where
    P: Destination<u16>,
{
    Peripheral::new_transfer(destination, channel, config)
}

impl<P, E> Peripheral<P, E>
where
    P: Source<E> + Destination<E>,
    E: Element,
{
    /// Wraps a peripheral that can act as both the source and destination of a DMA transfer
    pub fn new_transfer_receive(
        peripheral: P,
        tx: (Channel, Config),
        rx: (Channel, Config),
    ) -> Self {
        let mut peripheral = Peripheral::new(peripheral);
        peripheral.init_receive(rx.0, rx.1);
        peripheral.init_transfer(tx.0, tx.1);
        peripheral
    }

    /// Release the peripheral and both channels (transfer channel, release channel)
    ///
    /// Users should ensure that any active transfers are complete before releasing the
    /// peripheral.
    pub fn transfer_receive_release(mut self) -> (P, (Channel, Channel)) {
        (
            self.peripheral,
            (
                self.tx_channel.take().unwrap(),
                self.rx_channel.take().unwrap(),
            ),
        )
    }
}

/// Create a peripheral that can accept `u8` data from DMA transfers, and can
/// source `u8` data for DMA transfers
pub fn transfer_receive_u8<P>(
    peripheral: P,
    tx: (Channel, Config),
    rx: (Channel, Config),
) -> Peripheral<P, u8>
where
    P: Source<u8> + Destination<u8>,
{
    Peripheral::new_transfer_receive(peripheral, tx, rx)
}

/// Create a peripheral that can accept `u16` data from DMA transfers, and can
/// source `u16` data for DMA transfers
pub fn transfer_receive_u16<P>(
    peripheral: P,
    tx: (Channel, Config),
    rx: (Channel, Config),
) -> Peripheral<P, u16>
where
    P: Source<u16> + Destination<u16>,
{
    Peripheral::new_transfer_receive(peripheral, tx, rx)
}

/// Unclocked, uninitialized DMA channels
///
/// Use [`clock()`](struct.Unclocked.html#method.clock) to initialize and acquire all DMA channels
pub struct Unclocked([Option<Channel>; 32]);
impl Unclocked {
    pub(crate) fn new() -> Self {
        Unclocked([
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            None, None, None, None,
        ])
    }
    /// Enable the clocks for the DMA peripheral
    ///
    /// The return is 32 channels, each being initialized as `Some(Channel)`. Users may take channels as needed.
    /// The index in the array maps to the DMA channel number.
    pub fn clock(mut self, ccm: &mut ccm::Handle) -> [Option<Channel>; 32] {
        let (ccm, _) = ccm.raw();
        ral::modify_reg!(ral::ccm, ccm, CCGR5, CG3: 0x03);
        for (idx, channel) in self.0.iter_mut().enumerate() {
            *channel = Some(Channel::new(idx));
        }
        self.0
    }
}

/// A wrapper around a DMA error status value
///
/// The wrapper contains a copy of the DMA controller's
/// error status register at the point of an error. The
/// wrapper implements both `Debug` and `Display`. The
/// type may be printed to understand why there was a
/// DMA error.
#[derive(Clone, Copy)]
pub struct ErrorStatus {
    /// The raw error status
    es: u32,
}

impl ErrorStatus {
    fn new(es: u32) -> Self {
        ErrorStatus { es }
    }
}

impl Debug for ErrorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DMA_ES({:#010X})", self.es)
    }
}

impl Display for ErrorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,
            "DMA_ES: VLD {vld} ECX {ecx} GPE {gpe} CPE {cpe} ERRCHN {errchn} SAE {sae} SOE {soe} DAE {dae} DOE {doe} NCE {nce} SGE {sge} SBE {sbe} DBE {dbe}",
            vld = (self.es >> 31) & 0x1,
            ecx = (self.es >> 16) & 0x1,
            gpe = (self.es >> 15) & 0x1,
            cpe = (self.es >> 14) & 0x1,
            errchn = (self.es >> 8) & 0x1F,
            sae = (self.es >> 7) & 0x1,
            soe = (self.es >> 6) & 0x1,
            dae = (self.es >> 5) & 0x1,
            doe = (self.es >> 4) & 0x1,
            nce = (self.es >> 3) & 0x1,
            sge = (self.es >> 2) & 0x1,
            sbe = (self.es >> 1) & 0x1,
            dbe = self.es & 0x1
        )
    }
}
