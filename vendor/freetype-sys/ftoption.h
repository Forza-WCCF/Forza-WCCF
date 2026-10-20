/* FreeType's default options, less the compressed formats this build leaves out (no gzip, LZW module). */
#include <freetype/config/ftoption.h>
#undef FT_CONFIG_OPTION_USE_ZLIB
#undef FT_CONFIG_OPTION_USE_LZW
