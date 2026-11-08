# ext4 Compatibility Plan

## Goal

Mount, read, and write images produced by a stock `mke2fs -t ext4`
(e2fsprogs 1.47) read-write, with the same bar the ext3 work set: on-disk
results stay `e2fsck -fn` clean, and journal recovery works in both this
kernel and Linux/e2fsck, including after crashes under QEMU.

## Starting Point (end of the ext3 work)

- Accepted features: compat `has_journal ext_attr resize_inode dir_index`,
  incompat `filetype recover journal_dev(rejected at mount)`, ro-compat
  `sparse_super large_file`. Everything else is rejected with `EINVAL`.
- Block mapping is classic direct/indirect (`i_block[15]`), 32-bit block
  numbers throughout, 1-4 KiB blocks, 32-byte group descriptors.
- JBD2 log: v2 superblock, 32-bit tags, `INCOMPAT_REVOKE`, `COMPAT_CHECKSUM`
  (CRC32 commit blocks), multi-descriptor transactions, write barriers,
  ext3 commit/checkpoint semantics. Other JBD2 incompat bits are rejected.
- Test harness: `user/sample_ext3.img`, `tests/ext3_fsck.sh` (e2fsck
  cross-checks, crash-point images), `tests/musl_ext3_journal_smoke.sh` (QEMU
  SIGKILL + recovery).

## What Stock ext4 Adds

From `mke2fs -t ext4 -d ...` on e2fsprogs 1.47.0 (`dumpe2fs -h`):

| Feature | Class | On-disk change | Kernel impact |
| --- | --- | --- | --- |
| `extent` | incompat | `i_block` holds an extent tree (`EXT4_EXTENTS_FL` 0x80000) | New block-map backend: lookup, insert/split/merge, truncate, depth growth |
| `64bit` | incompat | 64-byte group descriptors (`s_desc_size`), `_hi` halves of block counts/locations | Descriptor accessor layer; 32-bit block numbers remain valid for filesystems below 16 TiB at 4 KiB blocks |
| `flex_bg` | incompat | Bitmaps/inode tables of a flex group packed into its first group | Locations already come from the GDT; the allocator must not assume per-group metadata placement |
| `metadata_csum` | ro-compat | crc32c on superblock, GDT, bitmaps (stored in GDT), inodes, extent blocks, directory leaf tails, htree tails, xattr blocks; seed from UUID / `s_checksum_seed` | Verify on read; compute on every metadata write; journal switches to JBD2 csum v3 once Linux mounts it |
| `huge_file` | ro-compat | `i_blocks` may count filesystem blocks (`EXT4_HUGE_FILE_FL`), 48-bit `i_blocks` | `i_blocks` accounting and stat |
| `dir_nlink` | ro-compat | Directories above 65000 subdirectories keep `i_links_count = 1` | Link counting in mkdir/rmdir/rename |
| `extra_isize` | ro-compat | `i_extra_isize` >= `s_min_extra_isize` (32): nanosecond timestamps, `crtime`, inode checksum high half | Inode init/write must fill the extra fields |
| (implied) | - | `bg_flags` `INODE_UNINIT`/`BLOCK_UNINIT`/`ITABLE_ZEROED`, `bg_itable_unused` (lazy init, enabled with `metadata_csum`/`uninit_bg`) | Allocator must initialize bitmaps of uninit groups instead of trusting their (unwritten) contents |
| (default) | - | 4 KiB blocks, 256-byte inodes, 4 MiB journal | Already supported sizes |

After Linux has mounted a `metadata_csum` filesystem, its journal gains
`INCOMPAT_64BIT` + `INCOMPAT_CSUM_V3` (16-byte tags with per-block checksums,
csum tails on descriptor/revoke/commit blocks). Recovering such a journal is
required even for a read-only mount when `needs_recovery` is set.

Explicitly out of scope until requested (keep rejecting):
`inline_data`, `encrypt`, `casefold`, `verity`, `ea_inode`, `mmp`,
`bigalloc`, `meta_bg`, `quota`, `project`, `orphan_file`, `fast_commit`,
`journal_dev`, `stable_inodes`, `largedir`.

## Code Seams (from a survey of `crates/fs`)

- Block mapping funnels through five functions in `crates/fs/src/lib.rs` /
  `ext2_alloc.rs`: `ext2_data_block_for_logical_index_state`,
  `ext2_ensure_data_block`, `ext2_detach_data_block`,
  `ext2_free_indirect_tree_blocks`, `ext2_count_indirect_tree_blocks_state`.
  An extent backend can live behind the same seam, selected per inode by
  `EXT4_EXTENTS_FL`. Journal credit estimators (`ext2_estimate_*`) need an
  extent-aware variant.
- `EXT2_GROUP_DESC_SIZE` (32) is used in 13 places; group descriptor fields
  are read at fixed offsets. A descriptor accessor that honours
  `s_desc_size` and `_hi` fields is the first refactor.
- Nothing reads `bg_flags`; nothing computes crc32c or crc16.
- `Ext2Inode` parses only the 128-byte core plus `i_file_acl`/`i_size_high`;
  extra fields are preserved but never written.
- The JBD2 tag parser assumes 8-byte tags (`parse_descriptor_tags`).

## Milestones

Each milestone ends with the full suite green (`make test`), `e2fsck -fn`
clean on every image the kernel wrote, and docs updated
(`docs/vfs.md`, this file, `docs/PORTING_PLAN.md`).

### M0 - Foundations (no new features accepted yet)

1. **Fixtures**: `scripts/build_ext4_sample.sh` producing
   `user/sample_ext4.img` (stock features) and `user/sample_ext4_nocsum.img`
   (`^metadata_csum,uninit_bg`), both split into four block groups
   (`-g 1024`) so flex_bg packing and uninit groups are present, with a
   depth-1 extent tree (20-extent fragmented file), a two-block htree
   directory, an xattr block, a fast symlink, and a 5 GiB sparse file.
2. **Group descriptor layer**: replace fixed 32-byte offsets with accessors
   (`desc_size`, block/inode bitmap, inode table, free counts, flags,
   checksums, `_hi` halves). Reject `64bit` filesystems whose high halves are
   non-zero (> 16 TiB) for now.
3. **crc32c + crc16** implementations with known-answer tests, and checksum
   helpers per structure (verify-only at this stage).
4. **Block-map seam**: route the five functions through a `BlockMap`
   abstraction with the existing indirect implementation as its only backend.
   Pure refactor; all ext2/ext3 tests must stay green.

Exit: no behaviour change; refactor covered by the existing 100+ fs tests.

**Status: done.**
- `scripts/build_ext4_sample.sh` builds both fixtures; they are committed
  (16 MiB each, ~20 KiB compressed).
- Superblock parsing is split into `ext2_parse_layout_with_len` (geometry,
  descriptor size; no feature judgement, so tests can inspect ext4 layouts)
  and `ext2_parse_meta_with_len` (layout + feature allowlist for mounting).
- `Ext2Meta::desc_size` (32, or `s_desc_size` with `64bit`, validated as a
  power of two in 64..=1024); `ext2_gd_location*` read lo/hi locations and
  refuse non-zero high halves with `EIO`; `ext2_gd_adjust_counter_state`
  updates 16+16-bit counters. All 13 fixed-size uses are gone.
- `crates/fs/src/ext4_csum.rs`: table-driven crc32c/crc16 and verifiers for
  the superblock, group descriptors (crc32c and crc16 variants), block/inode
  bitmaps, inodes (lo/hi with `extra_isize`), extent tree blocks, and
  directory leaf tails. Tests check every checksum `mkfs.ext4` wrote in both
  fixtures and that single-byte corruption is detected.
- `crates/fs/src/ext2_bmap.rs`: lookup/ensure/detach/free-all/count
  dispatch on `EXT4_EXTENTS_FL`; the indirect code is the only backend and an
  extent-flagged inode returns `EIO` (corruption on ext2/ext3).
- Both ext4 fixtures are still rejected at mount (`EINVAL`), as intended.

### M1 - Read-only ext4 mount

1. Accept `extent 64bit flex_bg huge_file dir_nlink extra_isize
   metadata_csum` but force the mount **read-only** (mutating VFS calls return
   `EROFS`) until M2/M3 land. Needs a read-only mount state in the VFS/ext2
   layer and `/proc/mounts` showing `ro`.
2. Extent tree read path: header/index/leaf parsing, any depth,
   unwritten extents read as zeros, holes, bounds checks.
3. `huge_file` `i_blocks` interpretation, nanosecond/`crtime` timestamps in
   `stat`.
4. `metadata_csum` verification on read (superblock, GDT, inode, extent
   block, directory leaf/htree tails); policy: `EIO` on mismatch with a
   serial log line.
5. JBD2 recovery of `INCOMPAT_64BIT` + `CSUM_V2/V3` journals (16-byte tags,
   tag checksums, descriptor/revoke/commit csum tails). Fixture: a journal
   crafted the way Linux writes it, cross-checked with `e2fsck`.

Exit: every file in the stock fixture reads back identical to what
`debugfs -R dump` produces; corrupted-checksum fixtures return `EIO`; a QEMU
smoke boots with an ext4 root mounted read-only and lists/reads files.

**Status: done.**
- **Mount policy.** `extent 64bit flex_bg csum_seed` (incompat) and
  `huge_file gdt_csum dir_nlink extra_isize metadata_csum` (ro-compat) are
  accepted and force `ext2_read_only`. Every VFS mutation on that mount
  returns `EROFS` (`backend_mutable` is state-aware; create, mkdir and
  `write` gained explicit guards). `/proc/mounts` shows `ext4 ... ro`, and
  `fs::ext2_fs_name()`/`ext2_mount_read_only()` feed the kernel's mount log.
  `gdt_csum` together with `metadata_csum` is rejected.
- **Extents.** `ext2_bmap.rs` walks extent trees of any depth up to 5 with
  bounds/depth/magic checks. Unwritten extents and holes read as zeros, and
  each tree block's checksum is verified. 48-bit physical blocks with
  non-zero high bits are refused (`EIO`).
- **Inodes.** `Ext2Inode` carries `ino`, `i_generation` and
  `l_i_blocks_high`. `stat` reports `huge_file` `i_blocks`, including
  `EXT4_HUGE_FILE_FL` (counted in filesystem blocks).
- **Checksums (EIO on mismatch + diagnostics hook).** The superblock and
  every group descriptor (crc32c or crc16) are checked at mount, before
  journal recovery. Inodes, extent tree blocks and directory leaf blocks are
  checked when read. `fs::set_ext2_diag_hook` lets the kernel print
  `[EXT4] <structure> checksum mismatch (id N)`.
- **Block 0.** With 4 KiB blocks, block 0 holds the superblock
  (`s_first_data_block = 0`) and is now a valid metadata block (latent ext2
  bug: every earlier fixture used 1 KiB blocks).
- **JBD2 recovery formats.** `64BIT`, `CSUM_V2` and `CSUM_V3` are understood:
  tag sizes follow `journal_tag_bytes`, 64-bit revoke records, descriptor
  and revoke tails, per-block data checksums, commit checksums, and journal
  superblock checksums (verified at mount, recomputed on write). Rules follow
  jbd2: a bad descriptor/revoke/data checksum fails recovery (here before
  anything is written), and a bad commit checksum ends the log. Transactions
  may start with a revoke block: a revoke-only transaction used to end replay
  early (latent bug). Journals with these features mount only read-only,
  because the writer emits the classic format. Read-only mounts never modify
  the journal superblock's features.
- **Superblock upkeep.** Every home write of the superblock block (journal
  checkpoints and plain cache writeback now share `backend_write_block_raw`)
  recomputes `s_checksum` on `metadata_csum` filesystems, so recovery leaves
  a valid superblock.
- **Tests.** Both fixtures read back completely (htree directory, depth-1
  extent file with holes, xattr file, fast symlink, 5 GiB sparse file);
  every mutation is refused with zero backend writes; one-byte corruptions
  of the superblock, descriptors (crc32c and crc16), an inode, an extent leaf
  and a directory block return `EIO`; `user/sample_ext4_journal.img`
  (debugfs-written `journal_64bit` + `journal_checksum_v3`, with a
  revoke-only transaction) replays to the same result as `e2fsck`, with
  corruption cases for data, descriptor, commit and journal superblock;
  `tests/ext3_fsck.sh` runs `e2fsck -fn` on the kernel-recovered image;
  `tests/musl_ext4_ro_smoke.sh` (`make run-ext4-ro-smoke`) boots QEMU from a
  stock ext4 root and checks reads, refusal of writes, `/proc/mounts`, and a
  byte-identical disk afterwards.
- **Deviations / gaps.** `stat` still reports whole seconds (nanoseconds and
  `crtime` are not decoded). htree `dx_tail` checksums and bitmap checksums
  are not verified (directory indexes are read linearly; bitmaps are only
  read by the allocator, which a read-only mount never runs). When a data
  block checksum fails, jbd2 still writes the other blocks before failing;
  this implementation fails before writing anything. The runtime `/etc/fstab`
  still says `ext3`. The QEMU smoke reuses the `musl-ext3-journal-smoke`
  init, whose staged script the runner replaces.

### M2 - Read-write on ext4 without `metadata_csum`

Target images: `mke2fs -t ext4 -O ^metadata_csum,uninit_bg`, the pre-1.43
e2fsprogs ext4 default (`user/sample_ext4_nocsum.img`): crc16 group
descriptor checksums (`gdt_csum`) and lazy-init groups, but no crc32c. On
e2fsprogs 1.47, `^metadata_csum` alone does not add `uninit_bg`.

1. Extent write path: allocate into the in-inode extent array, merge
   adjacent extents, split leaves, grow depth, free on truncate/unlink
   (including partial extents), journaled with extent-aware credits.
2. New inodes get `EXT4_EXTENTS_FL` when `extent` is enabled; directories too.
3. Allocator: `flex_bg`-agnostic placement, `BLOCK_UNINIT`/`INODE_UNINIT`
   initialization and flag clearing, `bg_itable_unused` maintenance, crc16
   group descriptor checksum (`gdt_csum`).
4. `extra_isize` initialization (timestamps, `crtime`), `dir_nlink` link
   counting.
5. Lift the read-only restriction for images without `metadata_csum`.

Exit: ext3-style mutation suite and crash-point suite pass on the
`^metadata_csum` fixture with `e2fsck -fn` clean; QEMU journal smoke runs on
an ext4 (`^metadata_csum`) root.

**Status: done.**
- **Read-write gating.** Only `metadata_csum` (ro-compat) and `csum_seed`
  (incompat) still force a read-only mount. Images with `extent 64bit flex_bg
  huge_file gdt_csum dir_nlink extra_isize` mount read-write as `ext4`.
- **Extent writes** (`ext2_bmap.rs`). Every change loads the inode's leaf
  extents into a sorted list, edits it, merges physically contiguous
  neighbours, and rewrites the tree: four or fewer extents live in the inode;
  more get leaf blocks (`eh_max` = (block size - 12) / 12) and, if needed,
  index levels (up to depth 5). Existing tree blocks are reused first, and
  leftovers are freed. Writes into unwritten extents split them, zero the
  block, and mark it initialized. Truncate/unlink detach or free blocks the
  same way. Credit estimates per step cover a full tree rewrite.
- **Allocator.** `ext2_alloc_block_goal` tries the block after the
  logically preceding extent first, then the rest of that group and the
  other groups. `BLOCK_UNINIT` groups get their bitmap built like
  `ext4_init_block_bitmap` (superblock/GDT backups with sparse_super,
  reserved GDT blocks, the group's own metadata when it lives inside it, bits
  past the end); the result must match `bg_free_blocks_count` or the
  operation fails with `EIO`. `INODE_UNINIT` bitmaps are zeroed with the tail
  set. `bg_itable_unused` shrinks as inodes past the initialized part are
  handed out. `bg_checksum` (crc16 or crc32c) is recomputed after every
  descriptor change.
- **Inodes.** New inodes get `EXT4_EXTENTS_FL` with an empty extent root and
  `i_extra_isize = s_want_extra_isize`. Directories get their first block
  through the block map. Inline symlinks drop `EXT4_EXTENTS_FL`. `dir_nlink`
  caps directory link counts at 1 past EXT4_LINK_MAX. Inodes with
  `EXT4_HUGE_FILE_FL` refuse writes like >4 GiB files.
- **Tests.** Sequential writes stay one extent; filling a hole in the depth-1
  tree, truncating it back into the inode, and unlinking restore exact free
  counts; interleaved writes grow two files to depth 1 and truncation shrinks
  one back; fast/slow symlinks and new directories use the ext4 layout; group
  1's `INODE_UNINIT` and `BLOCK_UNINIT` bitmaps are initialized on first use
  with valid crc16 descriptors; a synced mutation mix (including an htree
  insert, rename, unlink of the xattr and 5 GiB files) is `e2fsck -fn` clean
  in `tests/ext3_fsck.sh`; the reordering-cache crash test runs on the ext4
  image too (~244 recovered crash points, all `e2fsck` clean); and
  `tests/musl_ext4_journal_smoke.sh` runs the QEMU SIGKILL/recovery smoke on a
  writable ext4 root.
- **Gaps.** The whole-tree rewrite costs a tree walk per mapping change,
  which is fine for the small files PeetOS writes today (`TMPFS_MAX_FILE_SIZE`
  caps file size at 32 KiB) but not a final design. `crtime` and nanosecond
  timestamps are not set (left zero). Goal allocation only looks one block
  ahead; there is no Linux-style group/orlov placement. `s_free_blocks_count_hi`
  and other high halves are never updated (sizes below 2^32 blocks).

### M3 - `metadata_csum` read-write (stock ext4)

1. crc32c computed on every metadata write: superblock, GDT (crc32c low
   16 bits), block/inode bitmap checksums in the GDT, inode checksums
   (lo + hi with `extra_isize`), extent block tails, directory leaf tails
   (fake 12-byte dirent) and htree tails, xattr block checksums.
2. Checksum seed handling (`s_checksum_seed` / `metadata_csum_seed`).
3. Journal: write JBD2 `CSUM_V3` with `INCOMPAT_64BIT` tags (what Linux uses
   with `metadata_csum`), keeping `COMPAT_CHECKSUM` for non-csum journals.
4. Lift the read-only restriction for stock images.

Exit: full mutation, crash-point, and QEMU journal smoke suites on the stock
fixture; `e2fsck -fn` clean, including after `e2fsck -fy` replay of
kernel-written csum-v3 journals.

**Status: done.**
- **Read-write gating.** No supported feature forces a read-only mount any
  more: stock `mke2fs -t ext4` images (`metadata_csum`, optionally
  `metadata_csum_seed`) mount read-write. Read-only is now a mount option
  (`mount_ext2_block_backend_with_options(.., read_only)`, kernel feature
  `ext2-ro-root`), used by the M1 read-only smoke and tests.
- **Checksummed writes.** Every metadata write refreshes its checksum: inode
  csum lo/hi after each inode write; descriptor crc32c plus block/inode
  bitmap checksums (lo/hi with 64-byte descriptors) after each descriptor or
  bitmap change, including lazily initialized `BLOCK_UNINIT`/`INODE_UNINIT`
  groups; extent tree block tails; directory leaf tails (the 12-byte `0xDE`
  dirent) on every add/remove/`..` rewrite, with tails added to former htree
  blocks when `INDEX_FL` is cleared; xattr block checksums when a shared
  block's refcount drops; and `s_checksum` on every home write of the
  superblock block *and* on its journaled copy (Linux recovery replays the
  copy verbatim and then re-reads the superblock). New inodes get a random
  `i_generation`, which seeds their per-inode checksums.
- **Journal.** Read-write mounts of a `metadata_csum` filesystem switch the
  journal to JBD2 `CSUM_V3` with `64BIT` tags (16-byte tags, descriptor and
  revoke block tails, commit block crc32c, journal superblock checksum), as
  Linux does; other journals keep v1 `COMPAT_CHECKSUM`. A journal that is
  already v2/v3 keeps its format, so an ext3 journal switched to v3 gets v3
  transactions. Recovery now treats a data-block checksum failure as fatal
  only once the transaction's commit block is found: an uncommitted tail (a
  crash before the logged blocks were durable) ends the log, as in jbd2.
- **Fixes found on the way.** Renaming a directory across parents rewrote
  `..` in `i_block[0]` instead of the directory's first mapped block (wrong
  for extent directories).
- **Tests.** The stock image mounts read-write with a v3/64bit journal; a
  mutation mix (extent growth to depth 1, cross-parent directory rename, fast
  and slow symlinks, xattr and 5 GiB file unlinks, truncate, htree insert,
  `BLOCK_UNINIT` group use) leaves every superblock, descriptor, bitmap,
  inode, extent and directory checksum valid, remounts, and is `e2fsck -fn`
  clean; the debugfs-written csum-v3 journal fixture replays and then takes
  writes; the reordering-cache crash test runs on the stock image (~244
  recovered crash points, all `e2fsck` clean); an unrecovered crash image of
  kernel-written v3 transactions is replayed by `e2fsck -fy` (Linux jbd2
  recovery) into a clean filesystem with the committed file present; and
  `tests/musl_ext4_csum_journal_smoke.sh` runs the QEMU SIGKILL/recovery smoke
  on a stock ext4 root.
- **Gaps.** htree index blocks were not written yet (done in M4). xattr blocks are never created or edited beyond
  refcount drops. There is no `s_kbytes_written`/`s_wtime`-style superblock
  bookkeeping.

### M4 - Directory index maintenance

Insert into htree directories instead of dropping `INDEX_FL`: `half_md4`
(and `tea`, `legacy`) hashes with the superblock hash seed and
signed/unsigned variants, leaf splits, index growth, and checksummed
`dx_tail`. Lift the VFS per-directory child cap (`VFS_MAX_CHILDREN`) so large
directories are usable.

Exit: a directory with thousands of entries keeps a valid index (`e2fsck -fn`
reports no rehash needed) and lookups use it.

**Status: done.**
- **Hashes** (`crates/fs/src/ext2_htree.rs`). `legacy`, `half_md4` and `tea`,
  each with signed or unsigned name bytes (`s_flags`), seeded by
  `s_hash_seed`, checked against e2fsprogs `debugfs dx_hash` output.
- **Lookup.** Indexed directories are searched through the index (dx root,
  then a dx node when `indirect_levels` is 1, then one leaf plus any
  hash-collision continuation leaves). Index blocks are validated like
  Linux's `dx_probe` (root info, count/limit, block range); an index that
  fails validation is treated as linear, while a `dx_tail` checksum
  mismatch returns `EIO`. Unlinks find their leaf the same way.
- **Insert.** Entries go into the leaf for their hash. A full leaf is split
  at about half its size in hash order (the upper half moves to a new
  block, with the collision bit set when the split falls inside a run of
  equal hashes). A full index block is split into a new dx node, and a full
  root moves its entries into a new node and gains a level, up to two levels
  (Linux without `largedir`). A full one-block directory on a `dir_index`
  filesystem becomes an index (`make_indexed_dir`), with the root using
  `s_def_hash_version`. `metadata_csum` filesystems get `dx_tail` checksums
  on every index block change, and directory leaf tails on every leaf.
- **VFS.** The node table and each directory's child list grow on demand
  (`VFS_MAX_INODES` = 32768 live nodes; there is no per-directory cap any
  more), tmpfs data lives on the heap instead of in every node, and names
  may be 255 bytes. ext2-family path lookups import only the named entry
  (through the index); `readdir` imports the rest. `rmdir` checks emptiness
  on disk. `getcwd` and the console status line build paths from parent
  links instead of scanning directories from `/`, and `getdents64` records
  fit 255-byte names.
- **Tests.** Hash vectors for all six variants; 3000 entries on the stock
  ext4 image (index built from a one-block directory, a third unlinked, all
  checksums valid, remount lookups read one leaf each and stay lazy, a
  255-byte name); 700 links with 200-byte names on 1 KiB ext3 blocks grow a
  second index level and split dx nodes; all six hash variants build indexes
  from names with bytes >= 0x80. `tests/ext3_fsck.sh` runs `e2fsck -fn` on
  each of these, plus 2112 recovered crash points of inserts that split
  htree leaves. The QEMU journal smokes (ext3, ext4, stock ext4) sync a
  1500-entry directory before the crash; `debugfs htree` sees an index after
  e2fsck replay, and the recovered kernel resolves and lists every entry.
- **Gaps.** Three-level indexes (`largedir`) are not supported (the feature
  is still rejected at mount). Leaves are never merged and the index never
  shrinks after unlinks, as on Linux. `readdir` returns entries in creation
  order of the VFS list, not hash order, and fully imports the directory.
  VFS nodes are never evicted: every name looked up or listed since mount
  stays in memory until unlink or unmount, so `VFS_MAX_INODES` bounds the
  files touched per mount. Full paths are still limited to `VFS_MAX_PATH`
  (256 bytes).

### M5 - Hardening and remaining semantics

- 64-bit file sizes through the VFS (removes the 4 GiB `EOVERFLOW`/`EFBIG`
  limit) and `huge_file` > 2 TiB files.
- Orphan list processing at mount (`s_last_orphan`) and orphan handling for
  unlink-while-open; decide on `orphan_file` (newer e2fsprogs default).
- Timestamp extra epoch bits (post-2038), `i_version`, project/quota fields
  preserved.
- Optional: `fallocate` unwritten extents, `FIEMAP`-like introspection.

## Decisions (agreed 2026-11-08)

1. **Read-only first.** M1 ships a read-only ext4 mount before any write
   support.
2. **M2 before M3.** The extent write path is proven on `^metadata_csum`
   images before checksummed writes.
3. **Checksum mismatch on read returns `EIO`** (with a serial log line);
   there is no log-and-continue mode.
4. **`orphan_file`** stays rejected for now; whether M5 implements it is still
   open (e2fsprogs 1.47.0, used here, does not enable it by default).

## Limitations / Simplifications

- M0-M4 are implemented (stock ext4 mounts read-write, htree indexes are
  maintained); M5 is not started.
- The milestone list deliberately excludes the out-of-scope features listed
  above; images using them will keep failing to mount with `EINVAL`.
- 32-bit block numbers remain an internal limit (filesystems below 16 TiB at
  4 KiB blocks); true >16 TiB support is not planned.
- There is no Linux kernel in the loop: interoperability is checked with
  e2fsprogs (`e2fsck`, `debugfs`, `dumpe2fs`), which implements the same
  on-disk formats and journal recovery.
