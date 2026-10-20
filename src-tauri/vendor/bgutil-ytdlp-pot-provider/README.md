# bgutil-ytdlp-pot-provider 2.0.0 (yt-dlp plugin)

Unmodified copies of the yt-dlp plugin files from
https://github.com/Brainicism/bgutil-ytdlp-pot-provider (GPL-3.0).

Lynk writes them into its own plugin folder and points yt-dlp at it
(`--plugin-dirs`) only when retrying a YouTube download that was blocked; the
plugin then asks the local bgutil server (127.0.0.1:4416) for a PO token.
