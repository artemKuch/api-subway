from fastapi import Header


async def require_tenant(x_tenant: str = Header()):
    return x_tenant
