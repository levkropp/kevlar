# Memory Management

*This section will be expanded during Phase 2.*

Kevlar will use a VMAR/VMO (Virtual Memory Address Region / Virtual Memory Object) model
for address space management, replacing Kerla's flat `Vec<VmArea>` approach.
