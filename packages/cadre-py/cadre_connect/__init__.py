"""Cadre Connect, thin Python client (docs/CONNECT.md)."""
from .client import Cadre, CadreHosted, CadreError, b58encode, b58decode, sign_solana_tx, framed, DEVNET

__all__ = ["Cadre", "CadreHosted", "CadreError", "b58encode", "b58decode", "sign_solana_tx", "framed", "DEVNET"]
__version__ = "0.1.0"
