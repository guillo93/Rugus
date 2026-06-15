/* Raspberry Pi 3B+ (BCM2837, AArch64). La GPU carga kernel8.img en 0x80000 y
   salta ahí. .text.boot debe ir primero (contiene _start). */

ENTRY(_start)

SECTIONS
{
    . = 0x80000;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : { *(.rodata .rodata.*) }

    .data : { *(.data .data.*) }

    . = ALIGN(16);
    __bss_start = .;
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    . = ALIGN(16);
    __bss_end = .;

    /* Pila descendente desde 16 KiB por encima del fin de imagen. */
    . = ALIGN(16);
    . = . + 0x4000;
    _stack_top = .;

    /DISCARD/ : { *(.comment) *(.note.*) *(.ARM.attributes) }
}
