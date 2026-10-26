ENTRY(_start)

PHDRS
{
    ro PT_LOAD FILEHDR PHDRS FLAGS(4);
    text PT_LOAD FLAGS(5);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x00400000 + SIZEOF_HEADERS;
    .rodata : { *(.rodata .rodata.*) } :ro

    . = ALIGN(0x1000);
    .text : { *(.text._start) *(.text .text.*) } :text

    . = ALIGN(0x1000);
    .data : { *(.data .data.*) } :data
    .bss : { *(.bss .bss.* COMMON) } :data

    /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame.*) *(.note .note.*) }
}
