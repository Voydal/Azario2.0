import type { ParkingSearchWarning } from "../types/api";

interface WarningBannerProps {
  warnings: ParkingSearchWarning[];
}

export function WarningBanner({ warnings }: WarningBannerProps) {
  if (warnings.length === 0) {
    return null;
  }

  return (
    <div className="warnings" aria-label="Route warnings">
      {warnings.map((warning) => (
        <p className="warning" role="status" key={warning.code}>
          <strong>Route notice:</strong> {warning.message}
        </p>
      ))}
    </div>
  );
}
