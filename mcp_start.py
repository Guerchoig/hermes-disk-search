"""Точка входа MCP-сервера для Hermes: python mcp_start.py"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hds.mcp_server import run  # noqa: E402

if __name__ == "__main__":
    run()