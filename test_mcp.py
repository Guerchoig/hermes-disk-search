"""Smoke-тест MCP-сервера: подключение клиентом, список инструментов, вызов поиска."""
import asyncio
import os
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, ROOT)

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


async def main():
    server = StdioServerParameters(
        command=os.path.join(ROOT, ".venv", "Scripts", "python.exe"),
        args=[os.path.join(ROOT, "mcp_start.py")],
        cwd=ROOT,
    )
    async with stdio_client(server) as (read, write):
        async with ClientSession(read, write) as s:
            await s.initialize()
            tools = await s.list_tools()
            print("TOOLS:", [t.name for t in tools.tools])
            res = await s.call_tool("search_local_files", {"query": "1С:Документооборот"})
            print("TOOL RESULT:")
            print(res.content[0].text[:600])


asyncio.run(main())