export class SandkilnApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "SandkilnApiError";
    this.status = status;
  }
}
