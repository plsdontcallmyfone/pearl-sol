// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";

/// The EVM encoders against the CHAIN builder's own vectors (chain/agent-types/vectors/
/// bridge2.json, copied by vectors-gen/chain_bridge2.py into test/vectors/chain-bridge2.json;
/// its "source" field names the commit). These are the bytes BridgeUnlockV2 attesters sign
/// and BridgeLockV2 appends, so a mismatch here is a cross-side bug.
contract ChainBridge2Test is Fixture {
    string j;

    function setUp() public {
        setUpStack();
        j = vm.readFile("test/vectors/chain-bridge2.json");
    }

    function _u64(string memory k) internal view returns (uint64) {
        return uint64(vm.parseJsonUint(j, k));
    }

    function test_depositMessageMatchesChain() public view {
        bytes memory m = CadreFormats.depositMessage(
            uint8(vm.parseJsonUint(j, ".deposit.domain")),
            _u64(".deposit.chainId"),
            vm.parseJsonAddress(j, ".deposit.bridge"),
            _u64(".deposit.nonce"),
            vm.parseJsonBytes32(j, ".deposit.recipient"),
            _u64(".deposit.amount")
        );
        assertEq(m, vm.parseJsonBytes(j, ".deposit.message"));
    }

    function test_depositOrderMessageMatchesChain() public view {
        bytes memory m = CadreFormats.depositOrderMessage(
            uint8(vm.parseJsonUint(j, ".order.domain")),
            _u64(".order.chainId"),
            vm.parseJsonAddress(j, ".order.bridge"),
            _u64(".order.nonce"),
            vm.parseJsonBytes32(j, ".order.recipient"),
            _u64(".order.amount"),
            vm.parseJsonBytes32(j, ".order.bodyHash")
        );
        assertEq(m, vm.parseJsonBytes(j, ".order.message"));
    }

    function test_orderJobIdMatchesChain() public view {
        bytes32 id = CadreFormats.orderJobId(uint8(vm.parseJsonUint(j, ".order.domain")), _u64(".order.nonce"));
        assertEq(id, vm.parseJsonBytes32(j, ".order.jobId"));
    }

    function test_postJobBodyMatchesChain() public view {
        string memory f = ".order.bodyFields";
        CadreStorefront.JobSpec memory s;
        s.m = uint32(vm.parseJsonUint(j, string.concat(f, ".m")));
        s.n = uint32(vm.parseJsonUint(j, string.concat(f, ".n")));
        s.k = uint32(vm.parseJsonUint(j, string.concat(f, ".k")));
        s.config = vm.parseJsonBytes(j, string.concat(f, ".config"));
        s.hashA = vm.parseJsonBytes32(j, string.concat(f, ".hashA"));
        s.hashB = vm.parseJsonBytes32(j, string.concat(f, ".hashB"));
        s.dataUri = vm.parseJsonBytes(j, string.concat(f, ".dataUri"));
        s.deadlineBlocks = uint64(vm.parseJsonUint(j, string.concat(f, ".deadline")));
        s.maxBlocks = uint32(vm.parseJsonUint(j, string.concat(f, ".maxBlocks")));
        s.evaluators = vm.parseJsonBytes32Array(j, string.concat(f, ".evaluators"));
        s.threshold = uint8(vm.parseJsonUint(j, string.concat(f, ".threshold")));
        bytes memory body = store.postJobBody(
            s,
            uint64(vm.parseJsonUint(j, string.concat(f, ".workerPay"))),
            uint64(vm.parseJsonUint(j, string.concat(f, ".protocolFee")))
        );
        assertEq(body, vm.parseJsonBytes(j, ".order.body"));
        assertEq(sha256(body), vm.parseJsonBytes32(j, ".order.bodyHash"));
    }

    function test_withdrawalEntryMatchesChain() public {
        bytes memory e = vm.parseJsonBytes(j, ".withdrawal.entry");
        CadreFormats.Entry memory w = CadreFormats.parseEntry(e);
        assertEq(w.domain, vm.parseJsonUint(j, ".withdrawal.domain"));
        assertEq(w.nonce, vm.parseJsonUint(j, ".withdrawal.nonce"));
        assertEq(w.from, vm.parseJsonBytes32(j, ".withdrawal.from"));
        assertEq(w.recipient, vm.parseJsonBytes32(j, ".withdrawal.recipient"));
        assertEq(CadreFormats.toAddress(w.recipient), vm.parseJsonAddress(j, ".withdrawal.recipientEvm"));
        assertEq(w.amount, vm.parseJsonUint(j, ".withdrawal.amount"));
        assertEq(w.height, vm.parseJsonUint(j, ".withdrawal.height"));
        assertEq(CadreFormats.encodeEntry(w), e);

        // and the bridge pays it through the proof path
        vm.prank(admin);
        token.transfer(address(bridge), w.amount);
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        bytes32 h = postRoot(root, 77);
        address to = CadreFormats.toAddress(w.recipient);
        bridge.releaseProof(e, 0, proof, h);
        assertEq(token.balanceOf(to), w.amount);
    }
}
