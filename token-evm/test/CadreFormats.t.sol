// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {CadreToken} from "../src/CadreToken.sol";

/// The Solidity encoders against test/vectors/formats.json, written by an independent Rust
/// encoding in token-evm/vectors-gen (FORMATS.md is the spec both follow).
contract CadreFormatsTest is Fixture {
    string j;

    function setUp() public {
        setUpStack();
        j = vm.readFile("test/vectors/formats.json");
    }

    function _bridge() internal view returns (address) {
        return vm.parseJsonAddress(j, ".bridge");
    }

    function test_le() public pure {
        assertEq(CadreFormats.le64(0x0102030405060708), bytes8(0x0807060504030201));
        assertEq(CadreFormats.le32(0x01020304), bytes4(0x04030201));
        assertEq(CadreFormats.le16(0x0102), bytes2(0x0201));
    }

    function testFuzz_le64RoundTrip(uint64 v) public pure {
        bytes memory b = abi.encodePacked(CadreFormats.le64(v));
        assertEq(CadreFormats.readLe64(b, 0), v);
    }

    function test_depositMessage() public view {
        bytes memory m = CadreFormats.depositMessage(
            uint64(vm.parseJsonUint(j, ".chain_id")), _bridge(), uint64(vm.parseJsonUint(j, ".nonce")),
            vm.parseJsonBytes32(j, ".recipient"), uint64(vm.parseJsonUint(j, ".amount"))
        );
        assertEq(m, vm.parseJsonBytes(j, ".deposit_message"));
        assertEq(m.length, 16 + 1 + 8 + 20 + 8 + 32 + 8);
    }

    function test_orderMessageAndJobId() public view {
        bytes32 bodyHash = vm.parseJsonBytes32(j, ".body_hash");
        bytes memory m = CadreFormats.depositOrderMessage(
            46630, _bridge(), 5, vm.parseJsonBytes32(j, ".recipient"), 2_000_000, bodyHash
        );
        assertEq(m, vm.parseJsonBytes(j, ".deposit_order_message"));
        assertEq(CadreFormats.orderJobId(46630, _bridge(), 5), vm.parseJsonBytes32(j, ".order_job_id"));
    }

    function test_postJobBody() public view {
        CadreStorefront.JobSpec memory s = spec();
        bytes memory cfg = new bytes(52);
        for (uint256 i = 0; i < 52; i++) cfg[i] = 0x5a;
        s.config = cfg;
        s.hashA = bytes32(0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1);
        s.hashB = bytes32(0xb2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2);
        s.evaluators[0] = bytes32(0xe1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1);
        s.evaluators[1] = bytes32(0xe2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2);
        bytes memory body = store.postJobBody(s, 1_900_000_000, 100_000_000);
        assertEq(body, vm.parseJsonBytes(j, ".post_job_body"));
        assertEq(sha256(body), vm.parseJsonBytes32(j, ".body_hash"));
    }

    function test_entryV2() public view {
        bytes memory e = vm.parseJsonBytes(j, ".entry_v2");
        CadreFormats.Entry memory w = CadreFormats.parseEntry(e);
        assertEq(w.domain, 2);
        assertEq(w.nonce, 9);
        assertEq(w.from, bytes32(0x3333333333333333333333333333333333333333333333333333333333333333));
        assertEq(w.amount, 123_456);
        assertEq(w.height, 4242);
        assertTrue(CadreFormats.toAddress(w.recipient) != address(0));
        assertEq(CadreFormats.encodeEntry(w), e);
        assertEq(Sha256Mmr.appendElement(e), vm.parseJsonBytes(j, ".entry_v2_element"));
    }

    function testFuzz_entryRoundTrip(uint8 d, uint64 n, bytes32 f, address to, uint64 a, uint64 h) public pure {
        CadreFormats.Entry memory w = CadreFormats.Entry(d, n, f, CadreFormats.toBytes32(to), a, h);
        bytes memory e = CadreFormats.encodeEntry(w);
        assertEq(e.length, 90);
        CadreFormats.Entry memory r = CadreFormats.parseEntry(e);
        assertEq(r.domain, d);
        assertEq(r.nonce, n);
        assertEq(r.from, f);
        assertEq(CadreFormats.toAddress(r.recipient), to);
        assertEq(r.amount, a);
        assertEq(r.height, h);
    }

    function test_token() public {
        assertEq(token.decimals(), 6);
        assertEq(token.symbol(), "CADRE");
        assertEq(token.totalSupply(), SUPPLY);
        address u = makeAddr("u");
        vm.prank(admin);
        token.transfer(u, 100);
        vm.prank(u);
        token.burn(40);
        assertEq(token.totalSupply(), SUPPLY - 40);
        assertEq(token.balanceOf(u), 60);
        CadreToken t2 = new CadreToken(u, 5);
        assertEq(t2.balanceOf(u), 5);
    }
}
