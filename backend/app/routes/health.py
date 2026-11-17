from fastapi import APIRouter, Depends

from ..config import Settings, get_settings

router = APIRouter()


@router.get("/api/health")
def health(settings: Settings = Depends(get_settings)) -> dict:
    return {
        "status": "ok",
        "model": settings.claude_model,
        "api_key_configured": settings.api_key_configured,
        "max_upload_mb": settings.max_upload_mb,
        "max_pages": settings.max_pages,
    }
