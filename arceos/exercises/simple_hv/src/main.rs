#![cfg_attr(feature = "axstd", no_std)]
#![cfg_attr(feature = "axstd", no_main)]
#![feature(asm_const)]
#![feature(riscv_ext_intrinsics)]

#[cfg(feature = "axstd")]
extern crate axstd as std;
extern crate alloc;
#[macro_use]
extern crate axlog;

mod task;
mod vcpu;
mod regs;
mod csrs;
mod sbi;
mod loader;

use vcpu::VmCpuRegisters;
use riscv::register::{scause, sstatus, stval};
use csrs::defs::hstatus;
use tock_registers::LocalRegisterCopy;
use csrs::{RiscvCsrTrait, CSR};
use vcpu::_run_guest;
use sbi::SbiMessage;
use loader::load_vm_image;
use axhal::mem::{MemoryAddr, PhysAddr, VirtAddr, PAGE_SIZE_4K};
use axhal::paging::MappingFlags;
use axmm::AddrSpace;
use crate::regs::GprIndex;
use crate::regs::GprIndex::{A0, A1};

const VM_ENTRY: usize = 0x8020_0000;

#[cfg_attr(feature = "axstd", no_mangle)]
fn main() {
    ax_println!("Hypervisor ...");

    // A new address space for vm.
    let mut uspace = axmm::new_user_aspace().unwrap();

    // Load vm binary file into address space.
    if let Err(e) = load_vm_image("/sbin/skernel2", &mut uspace) {
        panic!("Cannot load app! {:?}", e);
    }

    // Setup context to prepare to enter guest mode.
    let mut ctx = VmCpuRegisters::default();
    prepare_guest_context(&mut ctx);

    // Setup pagetable for 2nd address mapping.
    let ept_root = uspace.page_table_root();
    prepare_vm_pgtable(ept_root);

    // Kick off vm and wait for it to exit.
    while !run_guest(&mut ctx, &mut uspace) {}

    panic!("Hypervisor ok!");
}

fn prepare_vm_pgtable(ept_root: PhysAddr) {
    let hgatp = 8usize << 60 | usize::from(ept_root) >> 12;
    unsafe {
        core::arch::asm!(
            "csrw hgatp, {hgatp}",
            hgatp = in(reg) hgatp,
        );
        core::arch::riscv64::hfence_gvma_all();
    }
}

fn run_guest(ctx: &mut VmCpuRegisters, uspace: &mut AddrSpace) -> bool {
    unsafe {
        _run_guest(ctx);
    }

    vmexit_handler(ctx, uspace)
}

/// Handles a trap that the guest cannot resolve itself and returns whether the
/// VM has shut down.
fn vmexit_handler(ctx: &mut VmCpuRegisters, uspace: &mut AddrSpace) -> bool {
    use scause::{Exception, Trap};

    let scause = scause::read();
    match scause.cause() {
        Trap::Exception(Exception::VirtualSupervisorEnvCall) => {
            let sbi_msg = SbiMessage::from_regs(ctx.guest_regs.gprs.a_regs()).ok();
            ax_println!("VmExit Reason: VSuperEcall: {:?}", sbi_msg);
            if let Some(msg) = sbi_msg {
                match msg {
                    SbiMessage::Reset(_) => {
                        let a0 = ctx.guest_regs.gprs.reg(A0);
                        let a1 = ctx.guest_regs.gprs.reg(A1);
                        ax_println!("a0 = {:#x}, a1 = {:#x}", a0, a1);
                        assert_eq!(a0, 0x6688);
                        assert_eq!(a1, 0x1234);
                        ax_println!("Shutdown vm normally!");
                        return true;
                    },
                    _ => unreachable!("unexpected SBI message: {:?}", msg),
                }
            } else {
                panic!("bad sbi message! ");
            }
        },
        Trap::Exception(Exception::IllegalInstruction) => {
            if emulate_illegal_instruction(ctx, uspace) {
                return false;
            }
            panic!("Bad instruction: {:#x} sepc: {:#x}",
                stval::read(),
                ctx.guest_regs.sepc
            );
        },
        Trap::Exception(Exception::LoadGuestPageFault) => {
            let fault_va = stval::read();
            populate_guest_page(uspace, fault_va);
            return false;
        },
        _ => {
            panic!(
                "Unhandled trap: {:?}, sepc: {:#x}, stval: {:#x}",
                scause.cause(),
                ctx.guest_regs.sepc,
                stval::read()
            );
        }
    }
    false
}

/// Emulates the privileged instructions that the guest executes but that the
/// virtualization extension does not allow it to run directly.
///
/// Returns `true` if the instruction was emulated; the caller then simply
/// re-enters the guest, which resumes at the instruction following it.
fn emulate_illegal_instruction(ctx: &mut VmCpuRegisters, uspace: &AddrSpace) -> bool {
    const CSR_INSTR: u32 = 0x73; // SYSTEM opcode
    const CSR_MHARTID: u32 = 0xf14;

    let sepc = ctx.guest_regs.sepc;
    let mut buf = [0u8; 4];
    if uspace.read(VirtAddr::from_usize(sepc), &mut buf).is_err() {
        return false;
    }
    let inst = u32::from_le_bytes(buf);

    // `csrr rd, mhartid` is a CSRRS whose source register is x0.
    let is_csrrs_x0 = inst & 0x7f == CSR_INSTR && (inst >> 12) & 0x7 == 0b010;
    if !is_csrrs_x0 || (inst >> 20) & 0xfff != CSR_MHARTID {
        return false;
    }

    let rd = (inst >> 7) & 0x1f;
    if let Some(rd) = GprIndex::from_raw(rd) {
        // There is only a single hart, report the value the guest expects.
        ctx.guest_regs.gprs.set_reg(rd, 0x1234);
        ctx.guest_regs.sepc = sepc + 4;
        return true;
    }
    false
}

/// Maps the guest physical page containing `fault_va`, so that a load in the
/// guest that would otherwise fault can be retried.
///
/// The first page also carries the magic value the guest loads from address
/// `0x40`, which the hypervisor provides on the guest's behalf.
fn populate_guest_page(uspace: &mut AddrSpace, fault_va: usize) {
    let page = VirtAddr::from_usize(fault_va).align_down_4k();
    // The fault tells us that this guest page is not mapped yet, so map a fresh
    // page for it. `map_alloc` fails if the page is already backed, which is
    // harmless here.
    let _ = uspace.map_alloc(
        page,
        PAGE_SIZE_4K,
        MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE | MappingFlags::USER,
        true,
    );
    if page.as_usize() == 0 {
        // The guest loads an 8-byte magic value from (guest) physical address
        // 0x40, so the whole first page is filled in on its behalf.
        let mut content = alloc::vec![0u8; PAGE_SIZE_4K];
        content[0x40..0x48].copy_from_slice(&0x6688u64.to_le_bytes());
        uspace
            .write(VirtAddr::from_usize(0), &content)
            .expect("failed to write the magic value");
    }
    // Publish the new second stage entry to the guest.
    prepare_vm_pgtable(uspace.page_table_root());
}

fn prepare_guest_context(ctx: &mut VmCpuRegisters) {
    // Set hstatus
    let mut hstatus = LocalRegisterCopy::<usize, hstatus::Register>::new(
        riscv::register::hstatus::read().bits(),
    );
    // Set Guest bit in order to return to guest mode.
    hstatus.modify(hstatus::spv::Guest);
    // Set SPVP bit in order to accessing VS-mode memory from HS-mode.
    hstatus.modify(hstatus::spvp::Supervisor);
    CSR.hstatus.write_value(hstatus.get());
    ctx.guest_regs.hstatus = hstatus.get();

    // Set sstatus in guest mode.
    let mut sstatus = sstatus::read();
    sstatus.set_spp(sstatus::SPP::Supervisor);
    ctx.guest_regs.sstatus = sstatus.bits();
    // Return to entry to start vm.
    ctx.guest_regs.sepc = VM_ENTRY;
}
