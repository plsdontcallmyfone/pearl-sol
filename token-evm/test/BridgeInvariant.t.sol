// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Test} from "forge-std/Test.sol";
import {Fixture} from "./utils/Fixture.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {CadreHeaderOracle} from "../src/CadreHeaderOracle.sol";
import {CadreToken} from "../src/CadreToken.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";

/// Drives random deposits, orders, signed and proved releases, refunds and replays.
contract BridgeHandler is Test {
    CadreBridge bridge;
    CadreStorefront store;
    CadreHeaderOracle oracle;
    CadreToken token;
    uint256[] signerKeys;
    uint256[] attesterKeys;
    uint256[] storeKeys;
    address user;

    mapping(uint64 => uint256) public paidCount;
    mapping(uint64 => uint256) public refundCount;
    uint64[] public orderNonces;
    uint256 public ghostReleased;
    uint256 public ghostRefunded;
    uint256 public ghostDeposited;
    uint256 public replaysAccepted;
    uint64 nextHeight = 1;

    constructor(
        CadreBridge b,
        CadreStorefront s,
        CadreHeaderOracle o,
        CadreToken t,
        uint256[] memory sk,
        uint256[] memory ak,
        uint256[] memory fk,
        address u
    ) {
        bridge = b;
        store = s;
        oracle = o;
        token = t;
        signerKeys = sk;
        attesterKeys = ak;
        storeKeys = fk;
        user = u;
    }

    function _sign(uint256[] storage keys, bytes32 digest) internal view returns (bytes[] memory sigs) {
        uint256[] memory ks = new uint256[](2);
        ks[0] = keys[0];
        ks[1] = keys[1];
        if (vm.addr(ks[1]) < vm.addr(ks[0])) (ks[0], ks[1]) = (ks[1], ks[0]);
        sigs = new bytes[](2);
        for (uint256 i = 0; i < 2; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(ks[i], digest);
            sigs[i] = abi.encodePacked(r, s, v);
        }
    }

    function deposit(uint64 amount) external {
        amount = uint64(bound(amount, 1, 1_000_000 * 1e6));
        vm.prank(user);
        bridge.deposit(amount, bytes32(uint256(1)));
        ghostDeposited += amount;
    }

    function order(uint256 eth) external {
        eth = bound(eth, 0.001 ether, 1 ether);
        vm.deal(user, eth);
        bytes32[] memory evs = new bytes32[](1);
        evs[0] = bytes32(uint256(5));
        CadreStorefront.JobSpec memory s = CadreStorefront.JobSpec(
            8, 8, 8, new bytes(52), 0, 0, "", 10, 1, evs, 1
        );
        vm.prank(user);
        uint64 n = store.buyJob{value: eth}(bytes32(uint256(2)), s, 0);
        orderNonces.push(n);
        ghostDeposited += bridge.deposits(n).amount;
    }

    function releaseSigned(uint64 nonce, uint64 amount) external {
        nonce = uint64(bound(nonce, 0, 40));
        uint256 vault = token.balanceOf(address(bridge));
        if (vault == 0) return;
        amount = uint64(bound(amount, 1, vault > type(uint64).max ? type(uint64).max : vault));
        address to = makeAddr("recipient");
        bytes[] memory sigs = _sign(signerKeys, bridge.releaseDigest(nonce, to, amount));
        bool was = bridge.isProcessed(nonce);
        try bridge.release(nonce, to, amount, sigs) {
            if (was) replaysAccepted++;
            paidCount[nonce]++;
            ghostReleased += amount;
        } catch {}
    }

    function releaseProved(uint64 nonce, uint64 amount) external {
        nonce = uint64(bound(nonce, 0, 40));
        uint256 vault = token.balanceOf(address(bridge));
        if (vault == 0) return;
        amount = uint64(bound(amount, 1, vault > type(uint64).max ? type(uint64).max : vault));
        address to = makeAddr("recipient");
        bytes memory e = CadreFormats.encodeEntry(
            CadreFormats.Entry(2, nonce, bytes32(0), CadreFormats.toBytes32(to), amount, nextHeight)
        );
        bytes memory el = Sha256Mmr.appendElement(e);
        bytes32 root = sha256(abi.encodePacked(uint64(1), sha256(abi.encodePacked(uint64(0), el))));
        bytes32 h = _post(root);
        bool was = bridge.isProcessed(nonce);
        try bridge.releaseProof(e, 0, hex"010000", h) {
            if (was) replaysAccepted++;
            paidCount[nonce]++;
            ghostReleased += amount;
        } catch {}
    }

    function _post(bytes32 root) internal returns (bytes32 h) {
        uint64 ht = nextHeight++;
        h = keccak256(abi.encode(root, ht));
        bytes[] memory sigs = _sign(attesterKeys, oracle.finalHeaderDigest(h, ht, 0, 0, root));
        oracle.postFinal(h, ht, 0, 0, root, sigs);
    }

    function refund(uint256 i) external {
        if (orderNonces.length == 0) return;
        uint64 n = orderNonces[i % orderNonces.length];
        CadreBridge.Deposit memory d = bridge.deposits(n);
        bytes[] memory sigs = _sign(signerKeys, bridge.refundDigest(n, d.refundTo, d.amount));
        try bridge.refundDeposit(n, sigs) {
            if (refundCount[n] > 0) replaysAccepted++;
            refundCount[n]++;
            ghostRefunded += d.amount;
            store.markRefunded(n);
        } catch {}
    }

    function finalize(uint256 i) external {
        if (orderNonces.length == 0) return;
        uint64 n = orderNonces[i % orderNonces.length];
        CadreStorefront.Order memory o = store.getOrder(n);
        bytes[] memory sigs = _sign(storeKeys, store.resultDigest(o.jobId, bytes32(uint256(1)), bytes32(uint256(2))));
        vm.prank(user);
        try store.finalizeOrder(n, bytes32(uint256(1)), bytes32(uint256(2)), "", sigs) {
            if (refundCount[n] > 0) replaysAccepted++; // finalized after refund
        } catch {}
    }

    function settle(uint256 i) external {
        if (orderNonces.length == 0) return;
        try store.settleShare(orderNonces[i % orderNonces.length]) {} catch {}
    }

    function ordersLength() external view returns (uint256) {
        return orderNonces.length;
    }
}

contract BridgeInvariantTest is Fixture {
    BridgeHandler handler;

    function setUp() public {
        setUpStack();
        address user = makeAddr("user");
        vm.prank(admin);
        token.transfer(user, 100_000_000 * 1e6);
        vm.prank(user);
        token.approve(address(bridge), type(uint256).max);
        handler = new BridgeHandler(bridge, store, oracle, token, signerKeys, attesterKeys, storeKeys, user);
        targetContract(address(handler));
    }

    /// Vault backing: the bridge holds exactly what was locked minus what was paid out.
    function invariant_vaultBacking() public view {
        assertEq(
            token.balanceOf(address(bridge)),
            bridge.totalDeposited() - bridge.totalReleased() - bridge.totalRefunded()
        );
        assertEq(bridge.totalDeposited(), handler.ghostDeposited());
        assertEq(bridge.totalReleased(), handler.ghostReleased());
        assertEq(bridge.totalRefunded(), handler.ghostRefunded());
    }

    /// No double release, no double refund, no finalize after a refund.
    function invariant_noReplay() public view {
        assertEq(handler.replaysAccepted(), 0);
        for (uint64 n = 0; n <= 40; n++) {
            assertLe(handler.paidCount(n), 1);
        }
    }

    /// The storefront holds exactly the held shares plus accrued (never-cranked) buyback ETH.
    function invariant_shareAccounting() public view {
        assertEq(address(store).balance, store.bbLocked() + store.bbAccrued() - store.bbSpent());
    }
}
