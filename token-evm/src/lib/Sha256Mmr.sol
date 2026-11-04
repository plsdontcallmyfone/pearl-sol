// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

/// Single-element inclusion proofs for the Cadre withdrawal log (`withdraw_root`): a
/// Commonware 2026.9.0 QMDB keyless database, an MMR over SHA-256 with the qmdb root
/// bagging (BackwardFold). Line-for-line port of crates/pearl-verify-sbf/src/mmr.rs
/// (`verify_inclusion`), using the SHA-256 precompile.
///
/// - leaf digest  = sha256(pos u64 BE || element)
/// - node digest  = sha256(pos u64 BE || left || right)
/// - fold(acc, p) = sha256(acc || p)
/// - root         = sha256(leaves u64 BE || [inactive u64 BE if nonzero] || folded peaks)
/// - proof codec  = varint leaves || varint inactive_peaks || varint count || digests
/// - a withdrawal leaf element = keyless Append(entry): 0x01 || varint(len) || entry
library Sha256Mmr {
    uint8 internal constant OK = 0;
    uint8 internal constant ERR_FORMAT = 1;
    uint8 internal constant ERR_PROOF = 2;
    uint8 internal constant ERR_ROOT = 3;

    error MmrFormat();
    error MmrProof();
    error MmrRoot();

    struct Peak {
        uint64 pos;
        uint32 height;
        uint64 leafStart;
    }

    struct Cursor {
        uint256 next;
        uint256 end;
        uint256 base; // byte offset of digest 0 inside the proof
        bool bad;
        uint64 loc;
        bytes element;
    }

    /// Reverts unless `proof` shows `element` at `loc` under `root`.
    function verifyInclusion(bytes memory proof, bytes memory element, uint64 loc, bytes32 root) internal view {
        uint8 e = check(proof, element, loc, root);
        if (e == ERR_FORMAT) revert MmrFormat();
        if (e == ERR_PROOF) revert MmrProof();
        if (e == ERR_ROOT) revert MmrRoot();
    }

    struct Cls {
        uint256 np;
        uint256 foldPrefix;
        uint256 prefixActive;
        uint256 after_;
        uint256 suffix;
        uint256 rangeIdx;
    }

    struct Parsed {
        bool ok;
        uint64 leaves;
        uint64 inactive;
        uint256 count;
        uint256 base;
    }

    /// Error code form (0 = included).
    function check(bytes memory proof, bytes memory element, uint64 loc, bytes32 root) internal view returns (uint8) {
        (uint8 e, bytes32 got) = _compute(proof, element, loc);
        if (e != OK) return e;
        return got == root ? OK : ERR_ROOT;
    }

    function _compute(bytes memory proof, bytes memory element, uint64 loc) private view returns (uint8, bytes32) {
        Parsed memory p;
        (p.ok, p.leaves, p.inactive, p.count, p.base) = parse(proof);
        if (!p.ok) return (ERR_FORMAT, 0);
        if (p.leaves == 0 || loc >= p.leaves || p.leaves >= (uint64(1) << 40)) return (ERR_PROOF, 0);
        Peak[] memory peaks = _peaks(p.leaves);
        Cls memory c = _classify(peaks, p.inactive, loc);
        if (p.inactive > c.np || c.rangeIdx == type(uint256).max) return (ERR_PROOF, 0);
        uint256 head = (c.foldPrefix > 0 ? 1 : 0) + c.prefixActive + c.after_ + (c.suffix > 0 ? 1 : 0);
        if (p.count < head) return (ERR_PROOF, 0);
        Cursor memory sib;
        sib.next = head;
        sib.end = p.count;
        sib.base = p.base;
        sib.loc = loc;
        sib.element = element;
        Peak memory rp = peaks[c.rangeIdx];
        bytes32 rangeDigest = _reconstruct(proof, rp.pos, rp.height, rp.leafStart, sib);
        if (sib.bad || sib.next != p.count) return (ERR_PROOF, 0);
        bytes32 got = _root(proof, p.base, c, rangeDigest, p.leaves, p.inactive);
        if (got == bytes32(0)) return (ERR_PROOF, 0);
        return (OK, got);
    }

    /// Bags the peak digests (BackwardFold) into the root; bytes32(0) if malformed.
    function _root(bytes memory proof, uint256 base, Cls memory c, bytes32 rangeDigest, uint64 leaves, uint256 inactive)
        private
        pure
        returns (bytes32)
    {
        uint256 foldCount = c.foldPrefix > 0 ? 1 : 0;
        uint256 suffixCount = c.suffix > 0 ? 1 : 0;
        bytes32[] memory list = new bytes32[](42);
        uint256 n;
        uint256 take;
        for (uint256 k = 0; k < foldCount + c.prefixActive; k++) {
            list[n++] = _digest(proof, base, take++);
        }
        list[n++] = rangeDigest;
        for (uint256 k = 0; k < c.after_ + suffixCount; k++) {
            list[n++] = _digest(proof, base, take++);
        }
        uint256 toFold =
            c.foldPrefix == 0 ? inactive : (inactive > c.foldPrefix ? inactive - c.foldPrefix : 0) + 1;
        (bool ok, bytes32 folded) = _fold(list, n, toFold);
        if (!ok) return bytes32(0);
        return inactive == 0
            ? sha256(abi.encodePacked(leaves, folded))
            : sha256(abi.encodePacked(leaves, uint64(inactive), folded));
    }

    /// Peaks left to right: one per set bit of `leaves`, highest first.
    function _peaks(uint64 leaves) private pure returns (Peak[] memory peaks) {
        peaks = new Peak[](40);
        uint256 np;
        uint64 accNodes;
        uint64 leafCursor;
        for (uint256 i = 40; i > 0; i--) {
            uint32 h = uint32(i - 1);
            if (leaves & (uint64(1) << h) != 0) {
                peaks[np++] = Peak(accNodes + (uint64(1) << (h + 1)) - 2, h, leafCursor);
                accNodes += (uint64(1) << (h + 1)) - 1;
                leafCursor += uint64(1) << h;
            }
        }
        assembly {
            mstore(peaks, np)
        }
    }

    /// BackwardFold classification of the peaks around `loc`.
    function _classify(Peak[] memory peaks, uint256 inactive, uint64 loc) private pure returns (Cls memory c) {
        c.np = peaks.length;
        c.rangeIdx = type(uint256).max;
        for (uint256 i = 0; i < peaks.length; i++) {
            uint64 ls = peaks[i].leafStart;
            uint64 le = ls + (uint64(1) << peaks[i].height);
            if (le <= loc) {
                if (i < inactive) c.foldPrefix++;
                else c.prefixActive++;
            } else if (ls > loc) {
                if (i >= inactive) c.suffix++;
                else c.after_++;
            } else {
                c.rangeIdx = i;
            }
        }
    }

    /// Forward-folds the first `toFold` digests, then backward-folds the rest onto them.
    function _fold(bytes32[] memory list, uint256 n, uint256 toFold) private pure returns (bool, bytes32) {
        bytes32 acc = list[0];
        uint256 j = 1;
        for (uint256 k = 1; k < toFold; k++) {
            if (j >= n) return (false, 0);
            acc = sha256(abi.encodePacked(acc, list[j]));
            j++;
        }
        if (j >= n) return (true, acc);
        bytes32 b = list[n - 1];
        uint256 m = n - 1;
        while (m > j) {
            m--;
            b = sha256(abi.encodePacked(list[m], b));
        }
        return (true, sha256(abi.encodePacked(acc, b)));
    }

    /// Parses the codec header; `base` is where the digests start. Rejects non-canonical
    /// varints, more than 128 digests and any length mismatch, like the Rust `parse`.
    function parse(bytes memory b)
        internal
        pure
        returns (bool ok, uint64 leaves, uint64 inactive, uint256 count, uint256 base)
    {
        uint256 pos;
        bool o1;
        bool o2;
        bool o3;
        uint64 c;
        (o1, leaves, pos) = readVarint(b, 0);
        if (!o1) return (false, 0, 0, 0, 0);
        (o2, inactive, pos) = readVarint(b, pos);
        if (!o2) return (false, 0, 0, 0, 0);
        (o3, c, pos) = readVarint(b, pos);
        if (!o3) return (false, 0, 0, 0, 0);
        if (c > 128 || b.length != pos + 32 * uint256(c)) return (false, 0, 0, 0, 0);
        return (true, leaves, inactive, c, pos);
    }

    function readVarint(bytes memory b, uint256 pos) internal pure returns (bool ok, uint64 value, uint256 next) {
        uint256 result;
        uint256 shift;
        while (true) {
            if (pos >= b.length) return (false, 0, pos);
            uint8 byte_ = uint8(b[pos]);
            pos++;
            if (shift == 63 && byte_ > 1) return (false, 0, pos);
            result |= uint256(byte_ & 0x7f) << shift;
            if (byte_ & 0x80 == 0) {
                if (byte_ == 0 && shift > 0) return (false, 0, pos); // non-canonical
                return (true, uint64(result), pos);
            }
            shift += 7;
            if (shift > 63) return (false, 0, pos);
        }
    }

    /// Keyless Append(entry) leaf element: 0x01 || varint(len) || entry.
    function appendElement(bytes memory entry) internal pure returns (bytes memory) {
        uint256 v = entry.length;
        bytes memory var_ = new bytes(10);
        uint256 n;
        while (true) {
            uint8 b = uint8(v & 0x7f);
            v >>= 7;
            if (v == 0) {
                var_[n++] = bytes1(b);
                break;
            }
            var_[n++] = bytes1(b | 0x80);
        }
        assembly {
            mstore(var_, n)
        }
        return abi.encodePacked(bytes1(0x01), var_, entry);
    }

    function _digest(bytes memory proof, uint256 base, uint256 i) private pure returns (bytes32 d) {
        uint256 off = base + 32 * i;
        assembly {
            d := mload(add(add(proof, 32), off))
        }
    }

    function _take(bytes memory proof, Cursor memory c) private pure returns (bytes32) {
        if (c.next >= c.end) {
            c.bad = true;
            return bytes32(0);
        }
        bytes32 d = _digest(proof, c.base, c.next);
        c.next++;
        return d;
    }

    /// DFS reconstruction of the peak containing `loc` (left first, siblings in visit order).
    function _reconstruct(bytes memory proof, uint64 pos, uint32 height, uint64 leafStart, Cursor memory sib)
        private
        view
        returns (bytes32)
    {
        if (sib.bad) return bytes32(0);
        uint64 leafEnd = leafStart + (uint64(1) << height);
        if (sib.loc < leafStart || sib.loc >= leafEnd) return _take(proof, sib);
        if (height == 0) return sha256(abi.encodePacked(pos, sib.element));
        bytes32 l = _reconstruct(proof, pos - (uint64(1) << height), height - 1, leafStart, sib);
        bytes32 r = _reconstruct(proof, pos - 1, height - 1, leafStart + (uint64(1) << (height - 1)), sib);
        return sha256(abi.encodePacked(pos, l, r));
    }
}
