import React from "react";
import { APP_NAME } from "@/hikaya/brand";

interface HikayaTextLogoProps {
  width?: number;
  className?: string;
}

/**
 * The product name as text. It stands in for Handy's drawn wordmark until
 * Hikaya has a logo of its own.
 */
const HikayaTextLogo: React.FC<HikayaTextLogoProps> = ({
  width,
  className = "",
}) => (
  <span
    role="img"
    aria-label={APP_NAME}
    style={{ width }}
    className={`block text-center text-2xl font-bold tracking-tight text-logo-primary select-none ${className}`}
  >
    {APP_NAME}
  </span>
);

export default HikayaTextLogo;
