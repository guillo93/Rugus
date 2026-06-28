/* Raspberry Pi 3B+ (BCM2837, AArch64). La GPU carga kernel8.img en 0x80000 y
   salta ahí. .text.boot debe ir primero (contiene _start).

   La sección `.user` (código/datos de las tareas EL0) se enlaza a VMA 0x200000
   —un bloque L2 de 2 MiB aparte del kernel, que el MMU mapea EL0 (AP=01, UXN=0)—
   sin AT(): la imagen incluye el hueco hasta 0x200000 (la GPU la carga contigua),
   así el código userland queda en 0x200000 al arrancar sin copia en runtime. */

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

    . = ALIGN(16);
    . = . + 0x4000;
    _stack_top = .;

    /* Userland EL0 a VMA 0x200000 (= LMA, sin AT). */
    . = 0x200000;
    __user_start = .;
    .user : {
        *(.user.text .user.text.*)
        *(.user.rodata .user.rodata.*)
        *(.user.data .user.data.*)
    }
    . = ALIGN(16);
    __user_end = .;

    /DISCARD/ : { *(.comment) *(.note.*) *(.ARM.attributes) }
}
