// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Test} from "forge-std/Test.sol";
import {PoolManager} from "v4-core/PoolManager.sol";
import {IPoolManager} from "v4-core/interfaces/IPoolManager.sol";
import {CadreToken} from "../../src/CadreToken.sol";
import {CadreBridge} from "../../src/CadreBridge.sol";
import {CadreHeaderOracle} from "../../src/CadreHeaderOracle.sol";
import {CadreStorefront} from "../../src/CadreStorefront.sol";
import {CadreReserve} from "../../src/CadreReserve.sol";
import {CadreV4Desk} from "../../src/CadreV4Desk.sol";
import {ICadreBridge, ICadreDesk} from "../../src/interfaces/ICadre.sol";

/// Full RH stack on a local EVM with the real v4-core v4.0.0 PoolManager. Every key here
/// is derived from a label (makeAddrAndKey), never an anvil default account.
abstract contract Fixture is Test {
    uint64 internal constant CADRE_CHAIN_ID = 1128350803;
    uint256 internal constant SUPPLY = 1_000_000_000 * 1e6;
    uint24 internal constant FEE = 3000;
    int24 internal constant SPACING = 60;
    /// 1 ETH = 1,000,000 CADRE: price (token base per wei) 1e12 / 1e18 = 1e-6, sqrt = 1e-3.
    uint160 internal constant SQRT_PRICE = 79228162514264337593543950;

    address internal admin;
    PoolManager internal manager;
    CadreToken internal token;
    CadreHeaderOracle internal oracle;
    CadreBridge internal bridge;
    CadreV4Desk internal desk;
    CadreReserve internal reserve;
    CadreStorefront internal store;

    uint256[] internal signerKeys; // bridge release/refund signers
    uint256[] internal attesterKeys; // header oracle attesters
    uint256[] internal storeKeys; // storefront attesters
    bytes32 internal miningPool = bytes32(uint256(0xC0FFEE));

    function _keys(string memory tag, uint256 n) internal returns (uint256[] memory ks, address[] memory as_) {
        ks = new uint256[](n);
        as_ = new address[](n);
        for (uint256 i = 0; i < n; i++) {
            (as_[i], ks[i]) = makeAddrAndKey(string.concat(tag, vm.toString(i)));
        }
    }

    function setUpStack() internal {
        admin = makeAddr("admin");
        (uint256[] memory sk, address[] memory sa) = _keys("signer", 3);
        (uint256[] memory ak, address[] memory aa) = _keys("attester", 3);
        (uint256[] memory fk, address[] memory fa) = _keys("store-attester", 3);
        signerKeys = sk;
        attesterKeys = ak;
        storeKeys = fk;

        vm.startPrank(admin);
        token = new CadreToken(admin, SUPPLY);
        manager = new PoolManager(admin);
        oracle = new CadreHeaderOracle(admin, CADRE_CHAIN_ID, aa, 2);
        bridge = new CadreBridge(admin, address(token), 2, sa, 2, 0);
        bridge.setReleaseModes(3, address(oracle));
        reserve = new CadreReserve(
            admin, address(token), ICadreBridge(address(bridge)), miningPool,
            CadreReserve.BuyLimits(address(0), address(0), 0, 0, 0, 300)
        );
        desk = new CadreV4Desk(admin, IPoolManager(address(manager)), address(token), FEE, SPACING, address(reserve));
        reserve.setBuyLimits(CadreReserve.BuyLimits(address(0), address(desk), 0, 0, 0, 300));
        desk.initializePool(SQRT_PRICE);
        vm.deal(admin, 1_000 ether);
        uint256 lpTokens = 100_000_000 * 1e6;
        token.approve(address(desk), lpTokens);
        uint128 liq = desk.liquidityForAmounts(100 ether, lpTokens);
        desk.addLiquidity{value: 100 ether}(liq, lpTokens);
        store = new CadreStorefront(
            admin, address(token), ICadreBridge(address(bridge)), ICadreDesk(address(desk)),
            CadreStorefront.Config({
                protocolFeeBps: 500,
                maxSlippageBps: 300,
                minEthIn: 0.0001 ether,
                maxEthIn: 10 ether,
                maxDim: 4096,
                maxBlocksCap: 64,
                maxDeadlineBlocks: 10_000
            }),
            CadreStorefront.Buyback({
                bps: 1000, dest: 0, reserve: address(reserve), cranker: address(0),
                maxPerCall: 0, minBuy: 0, minInterval: 600
            }),
            uint64(300_000_000 * 1e6),
            fa,
            2
        );
        bridge.setOrderSource(address(store));
        token.transfer(address(reserve), 200_000_000 * 1e6);
        vm.stopPrank();
    }

    /// Signs `digest` with `keys`, ordered by ascending signer address (what the contracts need).
    function sign(uint256[] memory keys, bytes32 digest) internal pure returns (bytes[] memory sigs) {
        uint256 n = keys.length;
        uint256[] memory ks = new uint256[](n);
        for (uint256 i = 0; i < n; i++) ks[i] = keys[i];
        for (uint256 i = 0; i < n; i++) {
            for (uint256 j = i + 1; j < n; j++) {
                if (vm.addr(ks[j]) < vm.addr(ks[i])) (ks[i], ks[j]) = (ks[j], ks[i]);
            }
        }
        sigs = new bytes[](n);
        for (uint256 i = 0; i < n; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(ks[i], digest);
            sigs[i] = abi.encodePacked(r, s, v);
        }
    }

    function firstN(uint256[] memory keys, uint256 n) internal pure returns (uint256[] memory out) {
        out = new uint256[](n);
        for (uint256 i = 0; i < n; i++) out[i] = keys[i];
    }

    function spec() internal pure returns (CadreStorefront.JobSpec memory s) {
        bytes32[] memory evs = new bytes32[](2);
        evs[0] = bytes32(uint256(0xe1));
        evs[1] = bytes32(uint256(0xe2));
        s = CadreStorefront.JobSpec({
            m: 64,
            n: 32,
            k: 2048,
            config: new bytes(52),
            hashA: bytes32(uint256(0xa1)),
            hashB: bytes32(uint256(0xb2)),
            dataUri: bytes("blob:abc"),
            deadlineBlocks: 100,
            maxBlocks: 8,
            evaluators: evs,
            threshold: 1
        });
    }

    /// A one-leaf withdrawal log holding `element`: (proof, root).
    function oneLeaf(bytes memory element) internal pure returns (bytes memory proof, bytes32 root) {
        bytes32 leaf = sha256(abi.encodePacked(uint64(0), element));
        root = sha256(abi.encodePacked(uint64(1), leaf));
        proof = hex"010000";
    }

    /// Posts a final header with `withdrawRoot` through the oracle; returns its hash.
    function postRoot(bytes32 withdrawRoot, uint64 height) internal returns (bytes32 h) {
        h = keccak256(abi.encode("hdr", withdrawRoot, height));
        bytes[] memory sigs = sign(firstN(attesterKeys, 2), oracle.finalHeaderDigest(h, height, 0, 0, withdrawRoot));
        oracle.postFinal(h, height, 0, 0, withdrawRoot, sigs);
    }
}
