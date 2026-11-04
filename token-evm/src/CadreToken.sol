// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {ERC20} from "solady/tokens/ERC20.sol";

/// CADRE on Robinhood Chain: fixed supply minted once to `holder`, 6 decimals (the same
/// base unit as the Solana SPL mint, so every bridge amount and invariant is unchanged).
/// No owner, no mint. Holders may burn their own tokens (the buyback burns above the
/// reserve cap, as Token-2022 BurnChecked does on Solana).
contract CadreToken is ERC20 {
    constructor(address holder, uint256 supply) {
        _mint(holder, supply);
    }

    function name() public pure override returns (string memory) {
        return "Cadre";
    }

    function symbol() public pure override returns (string memory) {
        return "CADRE";
    }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function burn(uint256 amount) external {
        _burn(msg.sender, amount);
    }
}
