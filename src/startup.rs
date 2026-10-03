//! Reset entry, exception routing, and debugger-visible failure records.
//!
//! Entry requires privileged Thread mode. Reset selects MSP, disables
//! maskable interrupts and SysTick, relocates the vectors, and stops enabled
//! DMA channels before copying .data and clearing .bss. Clock sources,
//! Flash timing, watchdogs, and external devices retain their existing state.
//!
//! Exceptions use a separate 1024-byte stack inside the reserved stack budget.
//! A hardware frame is copied only when it lies within the main stack range.
//! The failure record is excluded from reset clearing. Its payload checksum
//! is published last and validated before a boot copy is displayed.
//! Retention through the loader is observed, not guaranteed.

use core::arch::{asm, global_asm};
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use crate::{board, mcu};

extern "C" {
    static __stack_top: u8;
    static __main_stack_limit: u8;
}

#[repr(C)]
struct FaultRecord {
    signature: u32,
    exception: u32,
    exception_return: u32,
    frame_pointer: u32,
    frame_valid: u32,
    /// Hardware order: r0, r1, r2, r3, r12, lr, pc, xpsr.
    frame: [u32; 8],
    control: u32,
    interrupt_mask: u32,
    reason: u32,
}

#[used]
#[no_mangle]
#[link_section = ".noinit.trakt_fault"]
static mut TRAKT_FAULT: FaultRecord = unsafe { core::mem::zeroed() };

/// Copies the previous publication before invalidating it. The caller
/// validates the copy; reset does not treat retained RAM as initialized data.
pub(crate) fn take_previous_fault() -> [u32; 16] {
    let mut words = [0u32; 16];
    let source = addr_of!(TRAKT_FAULT) as *const u32;
    for (index, value) in words.iter_mut().enumerate() {
        *value = unsafe { read_volatile(source.add(index)) };
    }
    unsafe { write_volatile(addr_of_mut!(TRAKT_FAULT.signature), 0) };
    words
}

#[inline(never)]
fn publish_fault(record: FaultRecord) {
    let words = [
        0,
        record.exception,
        record.exception_return,
        record.frame_pointer,
        record.frame_valid,
        record.frame[0],
        record.frame[1],
        record.frame[2],
        record.frame[3],
        record.frame[4],
        record.frame[5],
        record.frame[6],
        record.frame[7],
        record.control,
        record.interrupt_mask,
        record.reason,
    ];
    let signature = crate::debug::seal(&words);
    unsafe {
        write_volatile(addr_of_mut!(TRAKT_FAULT.signature), 0);
        write_volatile(addr_of_mut!(TRAKT_FAULT), record);
        asm!("dsb", options(nostack, preserves_flags));
        write_volatile(addr_of_mut!(TRAKT_FAULT.signature), signature);
    }
}

/// Records a deliberate software stop without creating an exception frame.
/// Used only after RF shutdown by the watchdog recovery test.
pub(crate) fn remember_software(reason: u32) {
    publish_fault(FaultRecord {
        signature: 0,
        exception: 0,
        exception_return: 0,
        frame_pointer: 0,
        frame_valid: 0,
        frame: [0; 8],
        control: mcu::control(),
        interrupt_mask: mcu::interrupt_mask(),
        reason,
    });
}

pub(crate) fn main_stack_limit() -> u32 {
    addr_of!(__main_stack_limit) as usize as u32
}

/// Enters the common trap without returning through the current stack.
/// Reason 0 denotes an exception. Software stops use:
/// 0x1001 RAM, 0x1002 GPIO, 0x1003 panic, 0x1004 tick reload,
/// 0x1005 UART divisor, 0x1006 clock change, 0x1007 interrupt admission,
/// 0x1008 display transport initialization, 0x1009 ADC clock admission,
/// 0x100A external-memory transport initialization.
/// Clock decoding errors use 0x2000 plus the clock error number.
/// RF mismatch uses 0x3000 plus the register; 0x3147 is the AF challenge.
/// 0x6001 means a loaded snapshot failed application validation.
/// 0x6101 marks the deliberate watchdog test; 0x6102 is a stack guard
/// or sampled-margin violation; 0x6104 marks application-only reentry.
/// 0x7100 is the SysTick TX backstop. TX readback failures use 0x72xx;
/// calibration admission uses 0x73xx and forbidden TX writes use 0x74xx.
/// Watchdog errors: 0x4001 active WWDG, 0x4002 update timeout,
/// 0x4003 configuration mismatch, 0x4004 unsupported diagnostic HCLK.
pub(crate) fn stop(reason: u32) -> ! {
    unsafe {
        // BL reaches across the application image. The trap never returns.
        asm!("bl __trakt_stop", in("r0") reason, options(noreturn));
    }
}

#[no_mangle]
pub extern "C" fn __trakt_exception(
    frame_pointer: u32,
    exception_return: u32,
    exception: u32,
    reason: u32,
) -> ! {
    // Do not postpone RF shutdown behind recording or peripheral cleanup.
    unsafe {
        board::park_outputs();
        crate::drivers::bk4829::inhibit();
        mcu::write(0xE000_E010, 0);
        if mcu::read(mcu::RCC + 0x40) & (1 << 14) != 0 {
            mcu::write(0x4001_380C, 0);
        }
        if mcu::read(mcu::RCC + 0x38) & 1 != 0 {
            mcu::write(0x4002_001C, 0);
        }
        if mcu::read(mcu::RCC + 0x40) & (1 << 12) != 0 {
            mcu::write(0x4001_3000, 0);
        }
        if mcu::read(mcu::RCC + 0x40) & (1 << 9) != 0 {
            mcu::write(0x4001_2408, 0);
        }
        if mcu::read(mcu::RCC + 0x3C) & (1 << 14) != 0 {
            mcu::write(0x4000_3800, 0);
        }
    }
    let low = main_stack_limit();
    let high = addr_of!(__stack_top) as usize as u32;
    let frame_valid = matches!(
        exception_return,
        0xFFFF_FFF1 | 0xFFFF_FFF9 | 0xFFFF_FFFD
    ) && frame_pointer & 3 == 0
        && frame_pointer >= low
        && frame_pointer <= high - 32;

    let mut frame = [0u32; 8];
    if frame_valid {
        for (index, word) in frame.iter_mut().enumerate() {
            *word = unsafe {
                read_volatile((frame_pointer as *const u32).add(index))
            };
        }
    }

    let record = FaultRecord {
        signature: 0,
        exception,
        exception_return,
        frame_pointer,
        frame_valid: frame_valid as u32,
        frame,
        control: mcu::control(),
        interrupt_mask: mcu::interrupt_mask(),
        reason,
    };

    publish_fault(record);
    unsafe {
        board::park_outputs();
        mcu::write(mcu::GPIO_B + 0x18, 0x78);
        crate::drivers::bk4829::inhibit();
    }
    mcu::park()
}

global_asm!(
    r#"
    .syntax unified
    .thumb

    .section .vector_table,"a",%progbits
    .balign 256
    .global __trakt_vectors
    .type __trakt_vectors,%object
__trakt_vectors:
    .word __stack_top
    .word Reset_Handler
    .word Default_Handler
    .word Default_Handler
    .word 0, 0, 0, 0, 0, 0, 0
    .word Default_Handler
    .word 0, 0
    .word Default_Handler
    .word SysTick_Handler

    // IRQ 0..3: WWDG, PVD, RTC, FLASH.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 4..7: RCC/CTC, EXTI0_1, EXTI2_3, EXTI4_15.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 8..11: LCD, DMA channel 1, channels 2/3, channels 4/5/6/7.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 12..15: ADC/COMP, TIM1 update group, TIM1 capture, TIM2.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 16..19: TIM3, TIM6/LPTIM1/DAC, TIM7, TIM14.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 20..23: TIM15, TIM16, TIM17, I2C1.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 24..27: I2C2, SPI1, SPI2, USART1.
    .word Default_Handler, Default_Handler, Default_Handler, Default_Handler
    // IRQ 28..31: USART2, USART3/4, reserved, USB.
    .word Default_Handler, Default_Handler, 0, Default_Handler
    .size __trakt_vectors, .-__trakt_vectors

    .section .text.Reset_Handler,"ax",%progbits
    .balign 4
    .global Reset_Handler
    .type Reset_Handler,%function
    .thumb_func
Reset_Handler:
    mrs r0, IPSR
    cmp r0, #0
    beq .Lthread_entry
.Lbad_entry:
    b .Lbad_entry
.Lthread_entry:
    mrs r0, CONTROL
    movs r1, #1
    tst r0, r1
    bne .Lbad_entry

    cpsid i
    ldr r0, =__stack_top
    msr MSP, r0
    movs r0, #0
    msr CONTROL, r0
    isb

    ldr r1, =0xE000E010
    str r0, [r1]
    str r0, [r1, #4]
    str r0, [r1, #8]

    ldr r1, =0xE000E180
    ldr r2, =0xBFFFFFFF
    str r2, [r1]
    ldr r1, =0xE000E280
    str r2, [r1]

    ldr r1, =0xE000ED04
    ldr r2, =0x0A000000
    str r2, [r1]

    ldr r1, =0xE000ED08
    ldr r2, =__trakt_vectors
    str r2, [r1]

    ldr r1, =0xE000ED10
    ldr r2, [r1]
    movs r3, #6
    bics r2, r3
    str r2, [r1]
    dsb
    isb

    // DMA must stop before it can overwrite initialized application RAM.
    ldr r0, =0x40021038
    ldr r1, [r0]
    movs r2, #1
    tst r1, r2
    beq .Lcopy_data
    ldr r0, =0x40020008
    movs r1, #0
    str r1, [r0, #0]
    str r1, [r0, #20]
    str r1, [r0, #40]
    str r1, [r0, #60]
    str r1, [r0, #80]
    str r1, [r0, #100]
    str r1, [r0, #120]
    dsb

.Lcopy_data:
    // Reload does not start a disabled IWDG.
    ldr r0, =0x40003000
    ldr r1, =0x0000AAAA
    str r1, [r0]
    dsb

    ldr r0, =__data_load
    ldr r1, =__data_start
    ldr r2, =__data_end
.Lcopy_next:
    cmp r1, r2
    bhs .Lclear_bss
    ldr r3, [r0]
    str r3, [r1]
    adds r0, #4
    adds r1, #4
    b .Lcopy_next

.Lclear_bss:
    ldr r0, =__bss_start
    ldr r1, =__bss_end
    movs r2, #0
.Lclear_next:
    cmp r0, r1
    bhs .Lrun
    str r2, [r0]
    adds r0, #4
    b .Lclear_next

.Lrun:
    // No stack frame exists yet: the complete reservation can be painted.
    ldr r0, =__stack_limit
    ldr r1, =__stack_top
    ldr r2, =0xA55AC33C
.Lpaint_stack:
    cmp r0, r1
    bhs .Lpaint_guards
    str r2, [r0]
    adds r0, #4
    b .Lpaint_stack

.Lpaint_guards:
    ldr r2, =0x6B8D124F
    ldr r0, =__stack_limit
    str r2, [r0]
    str r2, [r0, #4]
    str r2, [r0, #8]
    str r2, [r0, #12]
    ldr r0, =__main_stack_limit
    str r2, [r0]
    str r2, [r0, #4]
    str r2, [r0, #8]
    str r2, [r0, #12]

    dsb
    bl __trakt_main
.Lreturned:
    b .Lreturned
    .ltorg
    .size Reset_Handler, .-Reset_Handler

    .section .text.trakt_trap,"ax",%progbits
    .balign 4
    .global Default_Handler
    .type Default_Handler,%function
    .thumb_func
Default_Handler:
    cpsid i
    mov r1, lr
    mrs r2, IPSR
    movs r3, #0
    movs r0, #4
    tst r0, r1
    beq .Lframe_msp
    mrs r0, PSP
    b .Ltrap_enter
.Lframe_msp:
    mrs r0, MSP
    b .Ltrap_enter
    .size Default_Handler, .-Default_Handler

    .global __trakt_stop
    .type __trakt_stop,%function
    .thumb_func
__trakt_stop:
    cpsid i
    mov r3, r0
    movs r0, #0
    movs r1, #0
    mrs r2, IPSR

    // Thread-mode stops must also select MSP before using the trap stack.
    movs r4, #0
    msr CONTROL, r4
    isb

.Ltrap_enter:
    ldr r4, =__fault_stack_top
    msr MSP, r4
    dsb
    bl __trakt_exception
.Ltrap_park:
    b .Ltrap_park
    .ltorg
    .size __trakt_stop, .-__trakt_stop
"#
);