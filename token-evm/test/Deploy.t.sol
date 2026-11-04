// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Test} from "forge-std/Test.sol";
import {PoolManager} from "v4-core/PoolManager.sol";
import {Deploy} from "../script/Deploy.s.sol";
import {CadreToken} from "../src/CadreToken.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {CadreV4Desk} from "../src/CadreV4Desk.sol";

/// Runs script/Deploy.s.sol in-process against a local PoolManager (no network) and
/// checks the wiring the plan's deploy order asks for.
contract DeployTest is Test {
    function test_deployScriptWiring() public {
        address deployer = makeAddr("deployer");
        vm.deal(deployer, 1 ether);
        PoolManager pm = new PoolManager(deployer);
        address[] memory a = new address[](3);
        for (uint256 i = 0; i < 3; i++) a[i] = makeAddr(string.concat("k", vm.toString(i)));
        string memory list = string.concat(vm.toString(a[0]), ",", vm.toString(a[1]), ",", vm.toString(a[2]));
        vm.setEnv("CADRE_CHAIN_ID", "1128350803");
        vm.setEnv("MINING_POOL", vm.toString(bytes32(uint256(0xC0FFEE))));
        vm.setEnv("ATTESTERS", list);
        vm.setEnv("SIGNERS", list);
        vm.setEnv("STORE_ATTESTERS", list);
        vm.setEnv("POOL_MANAGER", vm.toString(address(pm)));
        vm.setEnv("DEPLOY_OUT", "deployments/local-test.json");
        vm.setEnv("DEPLOYER", vm.toString(deployer));
        Deploy d = new Deploy();
        Deploy.Out memory o = d.run();

        CadreToken t = CadreToken(o.token);
        CadreBridge b = CadreBridge(o.bridge);
        assertEq(t.totalSupply(), 1_000_000_000 * 1e6);
        assertEq(t.balanceOf(o.reserve), 200_000_000 * 1e6);
        assertEq(b.releaseModes(), 3);
        assertEq(address(b.headers()), o.oracle);
        assertEq(b.orderSource(), o.store);
        assertEq(CadreV4Desk(payable(o.desk)).feeRecipient(), o.reserve);
        assertEq(address(CadreStorefront(payable(o.store)).desk()), o.desk);
        // the pool trades at the seeded price: 0.0005 ETH buys about 1M CADRE less impact
        uint256 q = CadreV4Desk(payable(o.desk)).quoteExactIn(true, 0.00001 ether);
        assertApproxEqRel(q, 20_000 * 1e6, 0.03e18);
        string memory json = vm.readFile("deployments/local-test.json");
        assertEq(vm.parseJsonAddress(json, ".bridge"), o.bridge);
    }
}
