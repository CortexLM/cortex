"""Source-host CLI for the installed qualification engine; no provider actions.

The package contains the implementation. Qualification also needs the explicitly
staged source tree/profile/jobs; this wrapper alone is not an image deployment.
"""

from hypertrain.gpu_ops.network_qualification import (
    Boundary as Boundary,
)
from hypertrain.gpu_ops.network_qualification import (
    Environment as Environment,
)
from hypertrain.gpu_ops.network_qualification import (
    Qualification as Qualification,
)
from hypertrain.gpu_ops.network_qualification import (
    Result as Result,
)
from hypertrain.gpu_ops.network_qualification import (
    bundle as bundle,
)
from hypertrain.gpu_ops.network_qualification import (
    capture as capture,
)
from hypertrain.gpu_ops.network_qualification import (
    check_inputs as check_inputs,
)
from hypertrain.gpu_ops.network_qualification import (
    compare as compare,
)
from hypertrain.gpu_ops.network_qualification import (
    inspect_environment as inspect_environment,
)
from hypertrain.gpu_ops.network_qualification import (
    main as main,
)
from hypertrain.gpu_ops.network_qualification import (
    run as run,
)
from hypertrain.gpu_ops.network_qualification import (
    sources as sources,
)

if __name__ == "__main__":
    main()
