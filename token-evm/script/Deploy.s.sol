// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Script, console2} from "forge-std/Script.sol";
import {IPoolManager} from "v4-core/interfaces/IPoolManager.sol";
import {CadreToken} from "../src/CadreToken.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {CadreHeaderOracle} from "../src/CadreHeaderOracle.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {CadreReserve} from "../src/CadreReserve.sol";
import {CadreV4Desk} from "../src/CadreV4Desk.sol";
import {ICadreBridge, ICadreDesk} from "../src/interfaces/ICadre.sol";

/// RH deploy, plan docs/RH-TESTNET-PLAN.md 4.6 steps 2-7. NOT RUN by the builder session.
///
/// Dry run (no broadcast, needs only the deployer ADDRESS):
///   forge script script/Deploy.s.sol --rpc-url rh_testnet --sender $DEPLOYER
/// Broadcast (owner's step; key from a file, never pasted):
///   forge script script/Deploy.s.sol --rpc-url rh_testnet --broadcast \
///     --private-key "$(cat keys/rh-testnet/deployer.key)" --sender $DEPLOYER
///
/// Env (all addresses are fresh keys, never anvil defaults):
///   CADRE_CHAIN_ID       Cadre genesis chain_id (u64)
///   MINING_POOL          bytes32 chain mining-pool account (reserve fund_mining recipient)
///   ATTESTERS            comma-separated header-oracle attester addresses; ATTESTER_THRESHOLD
///   SIGNERS              comma-separated bridge release/refund signers; SIGNER_THRESHOLD
///   STORE_ATTESTERS      comma-separated storefront result attesters; STORE_THRESHOLD
///   DEPLOYER             deployer address (defaults to --sender)
///   OWNER                admin (EOA on testnet, E11); defaults to the deployer
///   POOL_MANAGER         default canonical 0x8366a39cc670b4001a1121b8f6a443a643e40951
///   SUPPLY               default 1e15 (1B x 1e6, E1 (a) on testnet)
///   LP_TOKENS, LP_ETH    pool seed (default 1M CADRE + 0.0005 ETH: the start price; testnet only)
///   RESERVE_TOKENS       default 200M CADRE
///   BRIDGE_DOMAIN        chain bridge domain, default 2; a redeploy takes a NEW domain (2..=15)
///   MIN_WITHDRAWAL_NONCE refuse withdrawal nonces below this (0 for a new domain)
///   DEPLOY_OUT           where to write the JSON (default deployments/rh-testnet.json)
contract Deploy is Script {
    address constant CANONICAL_POOL_MANAGER = 0x8366a39CC670B4001A1121B8F6A443A643e40951;

    struct Out {
        address token;
        address oracle;
        address bridge;
        address desk;
        address reserve;
        address store;
    }

    struct Params {
        address sender;
        address owner;
        uint64 cadreChainId;
        bytes32 miningPool;
        address[] attesters;
        address[] signers;
        address[] storeAttesters;
        uint8 attesterThreshold;
        uint8 signerThreshold;
        uint8 storeThreshold;
        IPoolManager pm;
        uint256 supply;
        uint256 lpTokens;
        uint256 lpEth;
        uint256 reserveTokens;
        uint64 minNonce;
        uint8 domain;
    }

    function params() public view returns (Params memory p) {
        p.sender = vm.envOr("DEPLOYER", msg.sender);
        p.owner = vm.envOr("OWNER", p.sender);
        p.cadreChainId = uint64(vm.envUint("CADRE_CHAIN_ID"));
        p.miningPool = vm.envBytes32("MINING_POOL");
        p.attesters = vm.envAddress("ATTESTERS", ",");
        p.signers = vm.envAddress("SIGNERS", ",");
        p.storeAttesters = vm.envAddress("STORE_ATTESTERS", ",");
        p.attesterThreshold = uint8(vm.envOr("ATTESTER_THRESHOLD", uint256(2)));
        p.signerThreshold = uint8(vm.envOr("SIGNER_THRESHOLD", uint256(2)));
        p.storeThreshold = uint8(vm.envOr("STORE_THRESHOLD", uint256(2)));
        p.pm = IPoolManager(vm.envOr("POOL_MANAGER", CANONICAL_POOL_MANAGER));
        p.supply = vm.envOr("SUPPLY", uint256(1_000_000_000 * 1e6));
        p.lpTokens = vm.envOr("LP_TOKENS", uint256(1_000_000 * 1e6));
        p.lpEth = vm.envOr("LP_ETH", uint256(0.0005 ether));
        p.reserveTokens = vm.envOr("RESERVE_TOKENS", uint256(200_000_000 * 1e6));
        p.minNonce = uint64(vm.envOr("MIN_WITHDRAWAL_NONCE", uint256(0)));
        p.domain = uint8(vm.envOr("BRIDGE_DOMAIN", uint256(2)));
    }

    function run() external returns (Out memory o) {
        Params memory p = params();
        require(address(p.pm).code.length > 0, "no PoolManager at POOL_MANAGER");
        vm.startBroadcast(p.sender);
        _core(p, o);
        _market(p, o);
        _finish(p, o);
        vm.stopBroadcast();
        _write(o, address(p.pm), p.cadreChainId);
    }

    /// 2-4: token (all to the deployer), header oracle, bridge (modes signers + proof).
    function _core(Params memory p, Out memory o) internal {
        o.token = address(new CadreToken(p.sender, p.supply));
        o.oracle = address(new CadreHeaderOracle(p.sender, p.cadreChainId, p.attesters, p.attesterThreshold));
        CadreBridge bridge = new CadreBridge(p.sender, o.token, p.domain, p.signers, p.signerThreshold, p.minNonce);
        bridge.setReleaseModes(3, o.oracle);
        o.bridge = address(bridge);
    }

    /// 5-6: reserve first (the desk's fee recipient is immutable), desk + pool + seed, storefront.
    function _market(Params memory p, Out memory o) internal {
        CadreReserve reserve = new CadreReserve(
            p.sender, o.token, ICadreBridge(o.bridge), p.miningPool,
            CadreReserve.BuyLimits(address(0), address(0), uint128(0.05 ether), 0, 600, 300)
        );
        CadreV4Desk desk = new CadreV4Desk(p.sender, p.pm, o.token, 3000, 60, address(reserve));
        reserve.setBuyLimits(CadreReserve.BuyLimits(address(0), address(desk), uint128(0.05 ether), 0, 600, 300));
        desk.initializePool(sqrtPriceX96(p.lpTokens, p.lpEth));
        CadreToken(o.token).approve(address(desk), p.lpTokens);
        desk.addLiquidity{value: p.lpEth}(desk.liquidityForAmounts(p.lpEth, p.lpTokens), p.lpTokens);
        o.reserve = address(reserve);
        o.desk = address(desk);
        o.store = address(
            new CadreStorefront(
                p.sender, o.token, ICadreBridge(o.bridge), ICadreDesk(o.desk),
                CadreStorefront.Config({
                    protocolFeeBps: 500,
                    maxSlippageBps: 300,
                    minEthIn: 0.00001 ether,
                    maxEthIn: 0.1 ether,
                    maxDim: 4096,
                    maxBlocksCap: 64,
                    maxDeadlineBlocks: 10_000
                }),
                CadreStorefront.Buyback({
                    bps: 1000, dest: 0, reserve: o.reserve, cranker: address(0),
                    maxPerCall: uint128(0.05 ether), minBuy: 0, minInterval: 600
                }),
                uint64(100_000_000 * 1e6),
                p.storeAttesters,
                p.storeThreshold
            )
        );
    }

    /// Order source, 7: reserve share (the rest stays with the deployer for the owner's
    /// split), then E11: admin to OWNER.
    function _finish(Params memory p, Out memory o) internal {
        CadreBridge(o.bridge).setOrderSource(o.store);
        CadreToken(o.token).transfer(o.reserve, p.reserveTokens);
        if (p.owner != p.sender) {
            CadreHeaderOracle(o.oracle).transferOwnership(p.owner);
            CadreBridge(o.bridge).transferOwnership(p.owner);
            CadreReserve(payable(o.reserve)).transferOwnership(p.owner);
            CadreV4Desk(payable(o.desk)).transferOwnership(p.owner);
            CadreStorefront(payable(o.store)).transferOwnership(p.owner);
        }
    }

    /// sqrt(tokens / eth) in Q64.96 (currency0 = ETH, currency1 = CADRE).
    function sqrtPriceX96(uint256 tokens, uint256 eth) public pure returns (uint160) {
        // sqrt(tokens * 2^192 / eth)
        uint256 ratio = (tokens << 96) / eth;
        return uint160(sqrt(ratio) << 48);
    }

    function sqrt(uint256 x) internal pure returns (uint256 z) {
        if (x == 0) return 0;
        z = x;
        uint256 y = (x + 1) / 2;
        while (y < z) {
            z = y;
            y = (x / y + y) / 2;
        }
    }

    function _write(Out memory o, address pm, uint64 cadreChainId) internal {
        string memory k = "rh";
        vm.serializeUint(k, "chainId", block.chainid);
        vm.serializeUint(k, "cadreChainId", cadreChainId);
        vm.serializeAddress(k, "poolManager", pm);
        vm.serializeAddress(k, "token", o.token);
        vm.serializeAddress(k, "headerOracle", o.oracle);
        vm.serializeAddress(k, "bridge", o.bridge);
        vm.serializeAddress(k, "desk", o.desk);
        vm.serializeAddress(k, "reserve", o.reserve);
        string memory json = vm.serializeAddress(k, "storefront", o.store);
        string memory path = vm.envOr("DEPLOY_OUT", string("deployments/rh-testnet.json"));
        vm.writeJson(json, path);
        console2.log("wrote", path);
    }
}
