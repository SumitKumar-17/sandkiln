class SandkilnApiError(Exception):
    """Raised for any non-2xx response from the sandkiln daemon."""

    def __init__(self, status: int, message: str):
        super().__init__(f"{message} (status {status})")
        self.status = status
        self.message = message
