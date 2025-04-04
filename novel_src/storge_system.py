import yaml
from pathlib import Path
from typing import Dict, Any, Type, TypeVar, get_origin, get_args

T = TypeVar("T", bound="BaseConfig")


class ConfigError(Exception):
    """配置相关异常基类"""


class ConfigTypeError(ConfigError):
    """配置类型错误"""


class ConfigValidationError(ConfigError):
    """配置验证失败"""


class BaseConfigMeta(type):
    """元类用于收集配置字段信息"""

    def __new__(cls, name, bases, namespace):
        # 收集类型注解和默认值
        annotations = namespace.get("__annotations__", {})
        fields = {}

        for attr_name, attr_value in namespace.items():
            if attr_name.startswith("_") or not isinstance(attr_value, Field):
                continue

            # 从Field对象中提取信息
            field_info = {
                "type": annotations.get(attr_name, type(attr_value.default)),
                "default": attr_value.default,
                "description": attr_value.description,
            }
            fields[attr_name] = field_info

        # 创建类并保存字段信息
        new_cls = super().__new__(cls, name, bases, namespace)
        new_cls.__fields__ = fields
        return new_cls


class Field:
    """配置字段描述符"""

    def __init__(self, default: Any, description: str = ""):
        self.default = default
        self.description = description


class BaseConfig(metaclass=BaseConfigMeta):
    __config_path__: str = "config.yml"

    def __init__(self, **kwargs):
        for name, field in self.__fields__.items():
            value = kwargs.get(name, field["default"])
            setattr(self, name, value)

    @classmethod
    def load(cls: Type[T]) -> T:
        """从YAML加载配置"""
        config_path = Path(cls.__config_path__)

        if not config_path.exists():
            instance = cls()
            instance.save()
            return instance

        with open(config_path, "r", encoding="utf-8") as f:
            raw_data = yaml.safe_load(f)

        return cls(**cls._validate_config(raw_data))

    def save(self):
        """保存为带注释的YAML"""
        config_path = Path(self.__config_path__)
        config_path.parent.mkdir(parents=True, exist_ok=True)

        yaml_data = self._generate_yaml_with_comments()

        with open(config_path, "w", encoding="utf-8") as f:
            f.write(yaml_data)

    def _generate_yaml_with_comments(self) -> str:
        """生成带注释的YAML内容"""
        lines = []
        for name, field in self.__fields__.items():
            # 添加注释
            comment = field["description"].replace("\n", "\n# ")
            lines.append(f"# {comment}")

            # 添加字段值
            value = getattr(self, name)
            yaml_line = yaml.dump(
                {name: value}, default_flow_style=False, allow_unicode=True
            ).strip()
            lines.append(yaml_line)

        return "\n".join(lines)

    @classmethod
    def _validate_config(cls, raw_data: Dict[str, Any]) -> Dict[str, Any]:
        """验证并处理配置数据"""
        validated = {}

        for name, field_info in cls.__fields__.items():
            # 获取用户设置的值或使用默认值
            value = raw_data.get(name, field_info["default"])

            # 类型验证
            if not cls._check_type(value, field_info["type"]):
                raise ConfigTypeError(
                    f"字段 '{name}' 类型错误，应为 {field_info['type']}，实际为 {type(value)}"
                )

            validated[name] = value

        return validated

    @staticmethod
    def _check_type(value: Any, expected_type: Type) -> bool:
        """类型检查"""
        # 处理泛型类型（如Dict, List等）
        origin = get_origin(expected_type)
        if origin is None:
            return isinstance(value, expected_type)

        # 处理Dict类型
        if origin is dict:
            args = get_args(expected_type)
            key_type, value_type = args[0], args[1]
            return (
                isinstance(value, dict)
                and all(isinstance(k, key_type) for k in value.keys())
                and all(isinstance(v, value_type) for v in value.values())
            )

        # 可以在此添加其他泛型类型的处理
        return isinstance(value, origin)

    def __setattr__(self, name, value):
        """属性设置时的类型检查"""
        if name in self.__fields__:
            field_type = self.__fields__[name]["type"]
            if not self._check_type(value, field_type):
                raise ConfigTypeError(
                    f"字段 '{name}' 类型错误，应为 {field_type}，实际为 {type(value)}"
                )
        super().__setattr__(name, value)

    def update(self, **kwargs):
        """批量更新配置"""
        for name, value in kwargs.items():
            if name not in self.__fields__:
                raise ConfigError(f"无效配置项: {name}")
            setattr(self, name, value)

    def print_config(self):
        """打印当前配置"""
        print("当前配置：")
        for name, field_info in self.__fields__.items():
            print(f"{name} ({field_info['type'].__name__}):")
            print(f"  值: {getattr(self, name)}")
            if field_info["description"]:
                print(f"  描述: {field_info['description']}")
            print()
