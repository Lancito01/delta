# PawnIO modules

`IntelMSR.bin` and `AMDFamily17.bin` are unmodified, signed modules from
[PawnIO.Modules](https://github.com/namazso/PawnIO.Modules) release 0.2.11
(`release_0_2_11.zip`, SHA-256 `43608cb89bc84247fef1368a139013f7d043e17db6d6c8dfc9b46bf0905a81f4`).
Delta embeds them and hands them to the PawnIO driver to read the CPU temperature
(`src/metrics/cputemp.rs`). The driver only accepts modules signed by PawnIO's author, so they
can't be rebuilt or changed here.

They are Copyright (C) namazso and licensed under the GNU Lesser General Public License,
version 2.1 (see `COPYING`). Their source code is in the repository above, at tag `0.2.11`.

| File | SHA-256 |
|---|---|
| `IntelMSR.bin` | `d6ed85d65ab17a22f813ef98207d6d537155ee2ded5976a21cb48413c9b92e5f` |
| `AMDFamily17.bin` | `dae74615761b78bdf064dfb3e136252ddcc6fc727d88f14738d0e5800d427a91` |

The PawnIO driver itself is not part of Delta. It is installed, only on request, by PawnIO's own
installer (see `src/pawnio.rs`).
