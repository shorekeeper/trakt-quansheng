ENTRY(Reset_Handler)

MEMORY
{
    FLASH (rx) : ORIGIN = 0x08002800, LENGTH = 118K
    RAM (rw)   : ORIGIN = 0x20000000, LENGTH = 16K
}

PHDRS
{
    flash PT_LOAD FLAGS(5);
    data  PT_LOAD FLAGS(6);
    bss   PT_LOAD FLAGS(6);
    retained PT_LOAD FLAGS(6);
}

__stack_size = 8192;
__fault_stack_size = 1024;
__retained_size = 64;
__retained_start = ORIGIN(RAM) + LENGTH(RAM) - __retained_size;
__stack_top = __retained_start;
__stack_limit = __stack_top - __stack_size;
__fault_stack_top = __stack_limit + __fault_stack_size;
__main_stack_limit = __fault_stack_top;

SECTIONS
{
    .vector_table ORIGIN(FLASH) :
    {
        KEEP(*(.vector_table))
    } > FLASH :flash

    .text :
    {
        *(.text .text.*)
        . = ALIGN(4);
    } > FLASH :flash

    .rodata :
    {
        *(.rodata .rodata.*)
        *(.ARM.extab .ARM.extab.*)
        . = ALIGN(4);
    } > FLASH :flash

    .ARM.exidx :
    {
        __exidx_start = .;
        *(.ARM.exidx .ARM.exidx.*)
        __exidx_end = .;
        . = ALIGN(4);
    } > FLASH :flash

    .data :
    {
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4);
        __data_end = .;
    } > RAM AT> FLASH :data

    __data_load = LOADADDR(.data);
    __flash_used_end = LOADADDR(.data) + SIZEOF(.data);

    .bss (NOLOAD) :
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM :bss

    .noinit __retained_start (NOLOAD) :
    {
        __noinit_start = .;
        KEEP(*(.noinit .noinit.*))
        . = ALIGN(4);
        __noinit_end = .;
    } > RAM :retained

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame.*)
    }

    ASSERT(SIZEOF(.vector_table) == 192, "Expected 48 processor vectors.")
    ASSERT((ADDR(.vector_table) & 255) == 0, "Vector alignment is invalid.")
    ASSERT((__data_start & 3) == 0, "Data start is not word-aligned.")
    ASSERT((__data_end & 3) == 0, "Data end is not word-aligned.")
    ASSERT((__data_load & 3) == 0, "Data load address is not word-aligned.")
    ASSERT((__bss_start & 3) == 0, "BSS start is not word-aligned.")
    ASSERT((__bss_end & 3) == 0, "BSS end is not word-aligned.")
    ASSERT((__stack_top & 7) == 0, "Main stack top is not aligned.")
    ASSERT((__fault_stack_top & 7) == 0, "Fault stack top is not aligned.")
    ASSERT(__fault_stack_size >= 1024, "Fault stack reservation is too small.")
    ASSERT(__stack_size - __fault_stack_size >= 2048, "Main stack reservation is too small.")
    ASSERT(SIZEOF(.noinit) == 64, "Unexpected fault record size.")
    ASSERT(__bss_end <= __stack_limit, "Static RAM overlaps the stack budget.")
    ASSERT(__noinit_start == __stack_top, "Retained RAM overlaps the stack.")
    ASSERT(__noinit_end == ORIGIN(RAM) + LENGTH(RAM), "Retained RAM exceeds SRAM.")
    ASSERT(__flash_used_end <= ORIGIN(FLASH) + LENGTH(FLASH), "Flash capacity exceeded.")
}