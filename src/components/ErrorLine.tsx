type Props = { message: string | null };

/** Failure line. Never auto-dismisses; only a successful action clears it. */
export function ErrorLine({ message }: Props) {
  if (message === null) return null;
  return (
    <p role="alert" className="text-sm text-red-700">
      {message}
    </p>
  );
}
